//! Local NDJSON socket protocol between `beszel-relay` and desktop shell clients.
//!
//! This schema models the live (~1 Hz) metric stream only; any future historical
//! REST backfill will use separate types so cadences stay distinct.

use serde::{Deserialize, Serialize};

/// Hub connection lifecycle reported to local clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    Connecting,
    Connected,
    Paused,
    Reconnecting,
    Disconnected,
    AuthError,
}

/// Connection health metadata so widgets can render outages and staleness accurately.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionStatus {
    pub state: ConnectionState,
    /// Unix timestamp (ms) when `state` last changed.
    pub since_ms: i64,
    /// Unix timestamp (ms) of the most recently received live sample, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sample_ms: Option<i64>,
    /// Sanitized description of the latest connection error, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// Number of consecutive reconnect attempts since the last healthy connection.
    #[serde(default)]
    pub reconnect_attempts: u32,
}

/// Metadata for a subscribed Beszel system.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemSummary {
    pub id: String,
    pub name: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_version: Option<String>,
    /// Human-readable OS / distribution name (e.g. `"Fedora Linux 42"`, `"Ubuntu 24.04"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os_name: Option<String>,
    /// Normalized OS family (`"linux"`, `"macos"`, `"windows"`, `"freebsd"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
    /// Kernel release string, when reported by the hub.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kernel: Option<String>,
    /// CPU model string, when reported by the hub.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_model: Option<String>,
    /// CPU architecture (e.g. `"amd64"`, `"arm64"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
}

/// Single GPU telemetry snapshot within a `LiveSample`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GpuSample {
    pub name: String,
    pub usage_pct: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mem_used_mb: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mem_total_mb: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power_w: Option<f64>,
}

/// Single Docker/Podman container telemetry snapshot within a `LiveSample`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContainerSample {
    pub name: String,
    pub cpu_pct: f64,
    pub mem_mb: f64,
}

/// Single extra mounted filesystem snapshot within a `LiveSample`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiskSample {
    pub name: String,
    pub pct: f64,
    pub used_gb: f64,
    pub total_gb: f64,
}

/// Single live (~1 Hz) system telemetry sample stamped locally by `beszel-relay`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LiveSample {
    /// Local wall-clock receipt timestamp in milliseconds since Unix epoch.
    pub timestamp_ms: i64,
    /// Total CPU utilization percent (`0.0..=100.0`).
    pub cpu_pct: f64,
    /// Per-core CPU utilization percentages (`0..=100`), when reported by the agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_cores: Option<Vec<u8>>,
    /// Memory utilization percent (`0.0..=100.0`).
    pub mem_pct: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mem_used_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mem_total_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mem_buff_cache_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swap_used_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swap_total_bytes: Option<u64>,
    /// System uptime in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uptime_secs: Option<u64>,
    /// 1-, 5-, and 15-minute load averages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_avg: Option<[f64; 3]>,
    /// Highest/dashboard temperature in degrees Celsius.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temp_max_c: Option<f64>,
    /// Battery charge percentage (`0..=100`), when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battery_pct: Option<u8>,
    /// Battery charge state (`"charging"`, `"discharging"`, `"full"`, `"idle"`, `"empty"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battery_state: Option<String>,
    /// Count of failed systemd units when `> 0`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_services: Option<u16>,
    /// Total pending OS package updates when `> 0`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updates_total: Option<u16>,
    /// Pending security package updates when `> 0`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updates_security: Option<u16>,
    /// Aggregate network transmit rate in bytes/sec.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub net_sent_bps: Option<u64>,
    /// Aggregate network receive rate in bytes/sec.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub net_recv_bps: Option<u64>,
    /// Root filesystem utilization percent (`0.0..=100.0`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_pct: Option<f64>,
    /// Root filesystem used space in GiB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_used_gb: Option<f64>,
    /// Root filesystem total space in GiB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_total_gb: Option<f64>,
    /// Aggregate disk read rate in bytes/sec.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_read_bps: Option<u64>,
    /// Aggregate disk write rate in bytes/sec.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_write_bps: Option<u64>,
    /// Extra mounted filesystems reported by the agent (`stats.efs`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_disks: Option<Vec<DiskSample>>,
    /// Storage pools reporting a health state other than `"ONLINE"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unhealthy_pools: Option<Vec<String>>,
    /// Per-GPU live telemetry, when reported by the agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpus: Option<Vec<GpuSample>>,
    /// Top active containers sorted by CPU then memory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub containers: Option<Vec<ContainerSample>>,
}

/// Current live rolling window for one system delivered in initial snapshots.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SystemLiveState {
    pub system: SystemSummary,
    pub history_capacity: usize,
    /// Rolling live samples ordered oldest to newest.
    pub samples: Vec<LiveSample>,
}

/// NDJSON messages streamed from `beszel-relay` to connected local clients.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RelayEvent {
    /// Sent immediately upon client connect (and after lag recovery).
    Snapshot {
        protocol_version: u32,
        generated_at_ms: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hub_url: Option<String>,
        connection: ConnectionStatus,
        systems: Vec<SystemLiveState>,
    },
    /// Incremental ~1 Hz live sample for a subscribed system.
    Sample {
        system_id: String,
        sample: Box<LiveSample>,
    },
    /// Hub connection state transition.
    Connection { connection: ConnectionStatus },
    /// Updated metadata for subscribed systems.
    SystemsChanged { systems: Vec<SystemSummary> },
}

/// Encode a `RelayEvent` as a single NDJSON line (terminated by `\n`).
pub fn encode_ndjson(event: &RelayEvent) -> Result<String, serde_json::Error> {
    let mut line = serde_json::to_string(event)?;
    line.push('\n');
    Ok(line)
}

/// Decode a single NDJSON line into a `RelayEvent`.
pub fn decode_ndjson(line: &str) -> Result<RelayEvent, serde_json::Error> {
    serde_json::from_str(line.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LOCAL_PROTOCOL_VERSION;

    #[test]
    fn snapshot_round_trips_as_ndjson() {
        let event = RelayEvent::Snapshot {
            protocol_version: LOCAL_PROTOCOL_VERSION,
            generated_at_ms: 1_700_000_005_000,
            hub_url: Some("https://beszel.example.test".into()),
            connection: ConnectionStatus {
                state: ConnectionState::Connected,
                since_ms: 1_700_000_000_000,
                last_sample_ms: Some(1_700_000_004_900),
                last_error: None,
                reconnect_attempts: 0,
            },
            systems: vec![SystemLiveState {
                system: SystemSummary {
                    id: "sys_example_01".into(),
                    name: "workstation".into(),
                    status: "up".into(),
                    agent_version: Some("0.21.2".into()),
                    os_name: Some("Fedora Linux 42".into()),
                    os: Some("linux".into()),
                    ..SystemSummary::default()
                },
                history_capacity: 60,
                samples: vec![LiveSample {
                    timestamp_ms: 1_700_000_004_900,
                    cpu_pct: 4.95,
                    cpu_cores: Some(vec![4, 6, 3, 7]),
                    mem_pct: 13.45,
                    mem_used_bytes: Some(8_998_420_480),
                    mem_total_bytes: Some(66_916_192_256),
                    mem_buff_cache_bytes: Some(14_000_463_872),
                    swap_used_bytes: Some(0),
                    swap_total_bytes: Some(8_589_930_496),
                    net_sent_bps: Some(9_001),
                    net_recv_bps: Some(92_367),
                    ..LiveSample::default()
                }],
            }],
        };

        let encoded = encode_ndjson(&event).expect("encode should succeed");
        assert!(encoded.ends_with('\n'));
        assert!(!encoded[..encoded.len() - 1].contains('\n'));

        let decoded = decode_ndjson(&encoded).expect("decode should succeed");
        assert_eq!(decoded, event);
    }

    #[test]
    fn incremental_events_round_trip() {
        let events = [
            RelayEvent::Sample {
                system_id: "sys_example_01".into(),
                sample: Box::new(LiveSample {
                    timestamp_ms: 1_700_000_006_000,
                    cpu_pct: 3.67,
                    mem_pct: 13.44,
                    ..LiveSample::default()
                }),
            },
            RelayEvent::Connection {
                connection: ConnectionStatus {
                    state: ConnectionState::Reconnecting,
                    since_ms: 1_700_000_010_000,
                    last_sample_ms: Some(1_700_000_006_000),
                    last_error: Some("stream closed".into()),
                    reconnect_attempts: 1,
                },
            },
        ];

        for event in events {
            let line = encode_ndjson(&event).expect("encode should succeed");
            let decoded = decode_ndjson(&line).expect("decode should succeed");
            assert_eq!(decoded, event);
        }
    }
}
