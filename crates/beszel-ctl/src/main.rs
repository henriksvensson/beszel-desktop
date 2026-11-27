mod alerts;
mod alerts_api;
mod alerts_catalog;
mod format;
mod init;
mod update;

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use beszel_protocol::default_socket_path;
use beszel_protocol::local::{
    ActiveAlert, ConnectionState, ConnectionStatus, RelayEvent, SystemLiveState, decode_ndjson,
    encode_ndjson,
};
use clap::{Parser, Subcommand};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::UnixStream;

use crate::format::{ClientView, WaybarOptions, format_watch_sample, format_waybar, state_label};

/// Keep each `--raw` NDJSON stdout line well under downstream stream buffers (e.g. 64 KiB).
const MAX_RAW_LINE_BYTES: usize = 32 * 1024;

#[derive(Debug, Parser)]
#[command(name = "beszel-ctl", about = "Local socket client for beszel-relay")]
struct Args {
    /// Override the beszel-relay Unix socket path.
    #[arg(long, global = true)]
    socket: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Interactive setup wizard: configure hub credentials and start beszel-relay.
    Init(init::InitArgs),
    /// Manage Beszel hub alert rules (list, preset setup, set, remove).
    Alerts(alerts::AlertsArgs),
    /// Check for or install the latest verified GitHub Release binaries.
    Update(update::UpdateArgs),
    /// Connect, print the current snapshot and history count, then exit.
    Status,
    /// Stream live updates from beszel-relay.
    Watch {
        /// Print raw NDJSON lines directly from the Unix socket.
        #[arg(long)]
        raw: bool,
        /// Reconnect automatically if beszel-relay is unavailable or restarts.
        #[arg(long)]
        reconnect: bool,
    },
    /// Emit continuous Waybar custom-module JSON lines on stdout.
    Waybar {
        /// System ID or display name to display (defaults to first subscribed system).
        #[arg(long)]
        system: Option<String>,
        /// Mark data stale if no sample arrives for this many seconds.
        #[arg(long, default_value_t = 5)]
        stale_secs: u64,
        /// Emit the `warning` CSS class at or above this CPU/memory percentage.
        #[arg(long, default_value_t = 70.0)]
        warn_pct: f64,
        /// Emit the `danger` CSS class at or above this CPU/memory percentage.
        #[arg(long, default_value_t = 90.0)]
        danger_pct: f64,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let socket_path = args.socket.unwrap_or_else(default_socket_path);

    match args.command {
        Command::Init(opts) => init::run_init(opts, &socket_path).await,
        Command::Alerts(opts) => alerts::run_alerts(opts).await,
        Command::Update(opts) => update::run_update(opts).await,
        Command::Status => run_status(&socket_path).await,
        Command::Watch { raw, reconnect } => run_watch(&socket_path, raw, reconnect).await,
        Command::Waybar {
            system,
            stale_secs,
            warn_pct,
            danger_pct,
        } => {
            let opts = WaybarOptions {
                system_filter: system.as_deref(),
                stale_after_ms: (stale_secs as i64) * 1000,
                warn_pct,
                danger_pct,
            };
            run_waybar(&socket_path, opts).await
        }
    }
}

async fn run_status(socket_path: &Path) -> Result<()> {
    let stream = UnixStream::connect(socket_path)
        .await
        .with_context(|| format!("failed to connect to {}", socket_path.display()))?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await?;

    let event = decode_ndjson(&line).context("invalid snapshot from beszel-relay")?;
    let RelayEvent::Snapshot {
        protocol_version,
        connection,
        systems,
        ..
    } = event
    else {
        anyhow::bail!("expected initial Snapshot event from beszel-relay");
    };

    println!(
        "protocol=v{} state={} attempts={} systems={}",
        protocol_version,
        state_label(connection.state),
        connection.reconnect_attempts,
        systems.len()
    );
    for sys in systems {
        let latest = sys
            .samples
            .last()
            .map(|s| format!("cpu={:.1}% mem={:.1}%", s.cpu_pct, s.mem_pct))
            .unwrap_or_else(|| "no samples yet".into());
        println!(
            "  {} ({}) status={} history={}/{} {}",
            sys.system.name,
            sys.system.id,
            sys.system.status,
            sys.samples.len(),
            sys.history_capacity,
            latest
        );
    }
    Ok(())
}

async fn run_watch(socket_path: &Path, raw: bool, reconnect: bool) -> Result<()> {
    if !reconnect {
        return watch_single_connection(socket_path, raw).await;
    }

    loop {
        if let Err(err) = watch_single_connection(socket_path, raw).await {
            emit_watch_disconnected(raw, err.to_string())?;
        } else {
            emit_watch_disconnected(raw, "beszel-relay socket closed".into())?;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn watch_single_connection(socket_path: &Path, raw: bool) -> Result<()> {
    let stream = UnixStream::connect(socket_path)
        .await
        .with_context(|| format!("failed to connect to {}", socket_path.display()))?;
    let mut lines = BufReader::new(stream).lines();
    let mut system_names: Vec<(String, String)> = Vec::new();

    while let Some(line) = lines.next_line().await? {
        if raw {
            if !emit_raw_line(&line)? {
                return Ok(());
            }
            continue;
        }
        let event = decode_ndjson(&line).context("failed to decode relay event")?;
        print_watch_event(event, &mut system_names);
    }
    Ok(())
}

struct SnapshotMeta {
    protocol_version: u32,
    generated_at_ms: i64,
    hub_url: Option<String>,
    connection: ConnectionStatus,
    alerts: Vec<ActiveAlert>,
    alert_counts: BTreeMap<String, usize>,
}

fn emit_raw_line(line: &str) -> Result<bool> {
    if line.len() <= MAX_RAW_LINE_BYTES {
        return write_stdout_line(line);
    }
    let Ok(RelayEvent::Snapshot {
        protocol_version,
        generated_at_ms,
        hub_url,
        connection,
        systems,
        alerts,
        alert_counts,
    }) = decode_ndjson(line)
    else {
        return write_stdout_line(line);
    };

    let meta = SnapshotMeta {
        protocol_version,
        generated_at_ms,
        hub_url,
        connection,
        alerts,
        alert_counts,
    };
    emit_split_snapshot(meta, systems)
}

fn emit_split_snapshot(meta: SnapshotMeta, systems: Vec<SystemLiveState>) -> Result<bool> {
    let mut deferred = Vec::new();
    let header_systems = systems
        .into_iter()
        .map(|mut entry| {
            let samples = std::mem::take(&mut entry.samples);
            deferred.push((entry.system.id.clone(), samples));
            entry
        })
        .collect();

    let header = RelayEvent::Snapshot {
        protocol_version: meta.protocol_version,
        generated_at_ms: meta.generated_at_ms,
        hub_url: meta.hub_url,
        connection: meta.connection,
        systems: header_systems,
        alerts: meta.alerts,
        alert_counts: meta.alert_counts,
    };
    if !write_stdout_raw(&encode_ndjson(&header)?)? {
        return Ok(false);
    }
    for (system_id, samples) in deferred {
        for sample in samples {
            let event = RelayEvent::Sample {
                system_id: system_id.clone(),
                sample: Box::new(sample),
            };
            if !write_stdout_raw(&encode_ndjson(&event)?)? {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn write_stdout_line(line: &str) -> Result<bool> {
    let mut out = io::stdout().lock();
    match writeln!(out, "{line}") {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == io::ErrorKind::BrokenPipe => Ok(false),
        Err(err) => Err(err.into()),
    }
}

fn write_stdout_raw(text: &str) -> Result<bool> {
    let mut out = io::stdout().lock();
    match out.write_all(text.as_bytes()).and_then(|()| out.flush()) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == io::ErrorKind::BrokenPipe => Ok(false),
        Err(err) => Err(err.into()),
    }
}

fn emit_watch_disconnected(raw: bool, message: String) -> Result<()> {
    let connection = ConnectionStatus {
        state: ConnectionState::Disconnected,
        since_ms: now_ms(),
        last_sample_ms: None,
        last_error: Some(message),
        reconnect_attempts: 0,
    };
    if raw {
        let line = encode_ndjson(&RelayEvent::Connection { connection })?;
        print!("{line}");
    } else {
        println!(
            "state={} error={:?}",
            state_label(connection.state),
            connection.last_error
        );
    }
    Ok(())
}

fn print_watch_event(event: RelayEvent, system_names: &mut Vec<(String, String)>) {
    match event {
        RelayEvent::Snapshot {
            connection,
            systems,
            ..
        } => {
            *system_names = systems
                .iter()
                .map(|s| (s.system.id.clone(), s.system.name.clone()))
                .collect();
            println!("state={}", state_label(connection.state));
            for sys in systems {
                println!(
                    "snapshot {} ({}) history={}/{}",
                    sys.system.name,
                    sys.system.id,
                    sys.samples.len(),
                    sys.history_capacity
                );
                if let Some(last) = sys.samples.last() {
                    println!("{}", format_watch_sample(&sys.system.name, last));
                }
            }
        }
        RelayEvent::Sample { system_id, sample } => {
            let label = system_names
                .iter()
                .find(|(id, _)| id == &system_id)
                .map(|(_, name)| name.as_str())
                .unwrap_or(&system_id);
            println!("{}", format_watch_sample(label, &sample));
        }
        RelayEvent::Connection { connection } => {
            println!(
                "state={} attempts={} error={:?}",
                state_label(connection.state),
                connection.reconnect_attempts,
                connection.last_error
            );
        }
        RelayEvent::SystemsChanged { systems } => {
            *system_names = systems.into_iter().map(|s| (s.id, s.name)).collect();
        }
        RelayEvent::AlertsChanged { alerts, .. } => {
            println!("alerts={}", alerts.len());
        }
    }
}

async fn run_waybar(socket_path: &Path, opts: WaybarOptions<'_>) -> Result<()> {
    let mut view = ClientView::default();

    loop {
        match UnixStream::connect(socket_path).await {
            Ok(stream) => {
                pump_waybar_connection(stream, &mut view, opts).await?;
                view.connection = None;
                emit_waybar(&view, opts)?;
            }
            Err(_) => {
                view.connection = None;
                emit_waybar(&view, opts)?;
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn pump_waybar_connection(
    stream: UnixStream,
    view: &mut ClientView,
    opts: WaybarOptions<'_>,
) -> Result<()> {
    let mut lines = BufReader::new(stream).lines();
    let mut stale_tick = tokio::time::interval(Duration::from_secs(1));

    loop {
        tokio::select! {
            maybe_line = lines.next_line() => {
                let Some(line) = maybe_line? else {
                    return Ok(());
                };
                if let Ok(event) = decode_ndjson(&line) {
                    apply_event_to_view(view, event);
                    emit_waybar(view, opts)?;
                }
            }
            _ = stale_tick.tick() => {
                if view.connection.is_some() {
                    emit_waybar(view, opts)?;
                }
            }
        }
    }
}

fn apply_event_to_view(view: &mut ClientView, event: RelayEvent) {
    match event {
        RelayEvent::Snapshot {
            connection,
            systems,
            ..
        } => {
            let mapped = systems
                .into_iter()
                .map(|s| {
                    let last = s.samples.into_iter().last();
                    (s.system, last)
                })
                .collect();
            view.apply_snapshot(connection, mapped);
        }
        RelayEvent::Sample { system_id, sample } => {
            view.update_sample(&system_id, *sample);
        }
        RelayEvent::Connection { connection } => {
            view.connection = Some(connection);
        }
        RelayEvent::SystemsChanged { systems } => {
            view.systems = systems;
        }
        RelayEvent::AlertsChanged { .. } => {}
    }
}

fn emit_waybar(view: &ClientView, opts: WaybarOptions<'_>) -> Result<()> {
    let payload = format_waybar(view, opts, now_ms());
    println!("{}", serde_json::to_string(&payload)?);
    Ok(())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
