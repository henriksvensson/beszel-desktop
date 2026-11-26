//! Shared wire types for upstream Beszel realtime payloads and the local Unix socket stream.

pub mod local;
pub mod upstream;

use std::path::PathBuf;

/// Local NDJSON protocol version spoken between `beszel-relay` and clients.
pub const LOCAL_PROTOCOL_VERSION: u32 = 1;

/// Default filename for the user-only Unix domain socket.
pub const DEFAULT_SOCKET_NAME: &str = "beszel-relay.sock";

/// Resolve the default Unix socket path under `$XDG_RUNTIME_DIR`.
pub fn default_socket_path() -> PathBuf {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR")
        && !runtime_dir.is_empty()
    {
        return PathBuf::from(runtime_dir).join(DEFAULT_SOCKET_NAME);
    }
    std::env::temp_dir().join(DEFAULT_SOCKET_NAME)
}
