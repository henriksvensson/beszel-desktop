mod config;
mod hub;
mod server;
mod sleep;
mod sse;
mod state;

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use tokio::sync::watch;
use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::config::{CliOverrides, RelayConfig};
use crate::hub::HubSignals;
use crate::state::RelayState;

#[derive(Debug, Parser)]
#[command(
    name = "beszel-relay",
    about = "Per-user Beszel realtime metrics daemon"
)]
struct Args {
    /// Path to a TOML configuration file.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Beszel system ID or display name to subscribe to.
    #[arg(long)]
    system: Option<String>,
    /// Override the user-only Unix socket path.
    #[arg(long)]
    socket: Option<PathBuf>,
    /// Number of live (~1 Hz) samples retained per system in memory.
    #[arg(long)]
    history_capacity: Option<usize>,
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();

    let args = Args::parse();
    let config = RelayConfig::load(CliOverrides {
        config_path: args.config,
        system: args.system,
        socket_path: args.socket,
        history_capacity: args.history_capacity,
    })?;

    info!(?config, "Starting beszel-relay");

    let state = RelayState::new(config.history_capacity);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let wake_rx = sleep::spawn_wake_detector(shutdown_rx.clone());

    let hub_task = tokio::spawn(hub::run_hub_loop(
        config.clone(),
        state.clone(),
        HubSignals {
            shutdown: shutdown_rx.clone(),
            wake: wake_rx,
        },
    ));
    let server_task = tokio::spawn(server::run_unix_server(
        config.socket_path.clone(),
        state,
        shutdown_rx,
    ));

    wait_for_shutdown_signal().await;
    info!("Shutdown signal received");
    let _ = shutdown_tx.send(true);

    let (hub_res, server_res) = tokio::join!(hub_task, server_task);
    hub_res?;
    server_res??;
    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

async fn wait_for_shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut sigterm) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            sigterm.recv().await;
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
}
