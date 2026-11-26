use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use beszel_protocol::local::{RelayEvent, encode_ndjson};
use tokio::io::AsyncWriteExt;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, watch};
use tracing::{debug, info, warn};

use crate::state::RelayState;

/// Bind a user-only (`0o600`) Unix domain socket and serve NDJSON streams until shutdown.
pub async fn run_unix_server(
    socket_path: PathBuf,
    state: RelayState,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let listener = bind_user_socket(&socket_path)?;
    info!(socket = %socket_path.display(), "Listening on user-only Unix socket");

    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    break;
                }
            }
            accepted = listener.accept() => {
                spawn_accepted_client(accepted, &state, &shutdown);
            }
        }
    }

    cleanup_socket(&socket_path);
    Ok(())
}

fn spawn_accepted_client(
    accepted: std::io::Result<(UnixStream, tokio::net::unix::SocketAddr)>,
    state: &RelayState,
    shutdown: &watch::Receiver<bool>,
) {
    match accepted {
        Ok((stream, _addr)) => {
            let client_state = state.clone();
            let client_shutdown = shutdown.clone();
            tokio::spawn(async move {
                handle_client(stream, client_state, client_shutdown).await;
            });
        }
        Err(err) => {
            warn!(error = %err, "Failed to accept local Unix socket connection");
        }
    }
}

fn bind_user_socket(socket_path: &Path) -> Result<UnixListener> {
    if let Some(parent) = socket_path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create socket dir {}", parent.display()))?;
    }

    if socket_path.exists() {
        fs::remove_file(socket_path)
            .with_context(|| format!("failed to remove stale socket {}", socket_path.display()))?;
    }

    let listener = UnixListener::bind(socket_path)
        .with_context(|| format!("failed to bind Unix socket {}", socket_path.display()))?;
    fs::set_permissions(socket_path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("failed to chmod 0600 {}", socket_path.display()))?;
    Ok(listener)
}

fn cleanup_socket(socket_path: &Path) {
    if let Err(err) = fs::remove_file(socket_path)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        warn!(error = %err, socket = %socket_path.display(), "Failed to remove socket on exit");
    }
}

async fn handle_client(
    mut stream: UnixStream,
    state: RelayState,
    mut shutdown: watch::Receiver<bool>,
) {
    let _client_guard = state.register_client();
    let (snapshot, mut rx) = state.subscribe_with_snapshot();
    if write_event(&mut stream, &snapshot).await.is_err() {
        return;
    }
    debug!("Sent initial snapshot to local client");

    loop {
        let keep_open = tokio::select! {
            _ = shutdown.changed() => !*shutdown.borrow(),
            received = rx.recv() => forward_broadcast(&mut stream, &state, received).await,
        };
        if !keep_open {
            break;
        }
    }
}

async fn forward_broadcast(
    stream: &mut UnixStream,
    state: &RelayState,
    received: std::result::Result<RelayEvent, broadcast::error::RecvError>,
) -> bool {
    match received {
        Ok(event) => write_event(stream, &event).await.is_ok(),
        Err(broadcast::error::RecvError::Lagged(skipped)) => {
            debug!(
                skipped,
                "Local client lagged behind broadcast; resending snapshot"
            );
            let refreshed = state.snapshot();
            write_event(stream, &refreshed).await.is_ok()
        }
        Err(broadcast::error::RecvError::Closed) => false,
    }
}

async fn write_event(stream: &mut UnixStream, event: &RelayEvent) -> Result<()> {
    let line = encode_ndjson(event)?;
    stream.write_all(line.as_bytes()).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use beszel_protocol::local::{ConnectionState, LiveSample, SystemSummary, decode_ndjson};
    use tokio::io::{AsyncBufReadExt, BufReader};

    fn test_sample(timestamp_ms: i64, cpu_pct: f64, mem_pct: f64) -> LiveSample {
        LiveSample {
            timestamp_ms,
            cpu_pct,
            mem_pct,
            ..LiveSample::default()
        }
    }

    async fn wait_for_socket(socket_path: &Path) {
        for _ in 0..20 {
            if socket_path.exists() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn serves_user_only_socket_with_snapshot_and_updates() {
        let dir = std::env::temp_dir().join(format!("beszel-relay-test-{}", std::process::id()));
        let socket_path = dir.join("relay.sock");
        let state = RelayState::new(10);
        state.set_systems(vec![SystemSummary {
            id: "sys1".into(),
            name: "host-a".into(),
            status: "up".into(),
            agent_version: Some("0.21.2".into()),
            ..SystemSummary::default()
        }]);
        state.set_connection(ConnectionState::Connected, None, 0);
        state.record_sample("sys1", test_sample(1000, 5.0, 20.0));

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let server_task = tokio::spawn(run_unix_server(
            socket_path.clone(),
            state.clone(),
            shutdown_rx,
        ));
        wait_for_socket(&socket_path).await;

        let mode = fs::metadata(&socket_path)
            .expect("socket should exist")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);

        let stream = UnixStream::connect(&socket_path).await.expect("connect");
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).await.expect("snapshot line");
        assert!(matches!(
            decode_ndjson(&line).expect("snapshot"),
            RelayEvent::Snapshot { .. }
        ));

        state.record_sample("sys1", test_sample(2000, 9.5, 21.0));
        line.clear();
        reader.read_line(&mut line).await.expect("sample line");
        assert!(matches!(
            decode_ndjson(&line).expect("sample"),
            RelayEvent::Sample { .. }
        ));

        let _ = shutdown_tx.send(true);
        server_task.await.expect("join").expect("clean exit");
        let _ = fs::remove_dir_all(&dir);
    }
}
