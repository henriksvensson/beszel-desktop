//! Defensive wire types for Beszel / PocketBase HTTP and SSE payloads.

use serde::{Deserialize, Serialize};

use crate::local::LiveSample;

const GIB_BYTES: f64 = 1024.0 * 1024.0 * 1024.0;

/// PocketBase realtime topic for watching changes to the `systems` collection.
pub const SYSTEMS_COLLECTION_TOPIC: &str = "systems/*";

/// PocketBase realtime event payload emitted for `systems/*`.
#[derive(Debug, Clone, Deserialize)]
pub struct SystemsRealtimeEvent {
    pub action: String,
    pub record: UpstreamSystemRecord,
}

/// PocketBase `POST /api/collections/users/auth-with-password` response.
#[derive(Debug, Clone, Deserialize)]
pub struct AuthResponse {
    pub token: String,
}

/// PocketBase `GET /api/collections/systems/records` paginated response.
#[derive(Debug, Clone, Deserialize)]
pub struct SystemsListResponse {
    #[serde(default)]
    pub items: Vec<UpstreamSystemRecord>,
}

/// System record returned by Beszel's `systems` collection.
#[derive(Debug, Clone, Deserialize)]
pub struct UpstreamSystemRecord {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub info: Option<UpstreamSystemInfo>,
}

/// PocketBase `PB_CONNECT` SSE event payload.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PbConnectEvent {
    pub client_id: String,
}

/// PocketBase `POST /api/realtime` subscription request body.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RealtimeSubscribeRequest {
    pub client_id: String,
    pub subscriptions: Vec<String>,
}

/// Live `rt_metrics` SSE payload emitted by Beszel hub (~1 Hz).
#[derive(Debug, Clone, Deserialize)]
pub struct RtMetricsPayload {
    #[serde(default)]
    pub stats: Option<UpstreamStats>,
    #[serde(default)]
    pub info: Option<UpstreamSystemInfo>,
}

/// Summary fields carried on system records and `rt_metrics.info`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct UpstreamSystemInfo {
    /// Agent semantic version (e.g. `"0.21.2"`).
    #[serde(default)]
    pub v: Option<String>,
    /// System uptime in seconds.
    #[serde(default)]
    pub u: Option<u64>,
    /// CPU percent summary.
    #[serde(default)]
    pub cpu: Option<f64>,
    /// Memory percent summary.
    #[serde(default)]
    pub mp: Option<f64>,
}

/// Detailed live system metrics carried in `rt_metrics.stats`.
///
/// Upstream Go structs use `omitzero` / `omitempty` extensively; all fields
/// remain optional so minor Beszel releases or zero ticks decode cleanly.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct UpstreamStats {
    /// Total CPU utilization percent.
    #[serde(default)]
    pub cpu: Option<f64>,
    /// Per-core busy utilization percentages (`[CPU0..]`).
    #[serde(default)]
    pub cpus: Option<Vec<u8>>,
    /// Memory utilization percent.
    #[serde(default)]
    pub mp: Option<f64>,
    /// Byte-accurate memory `[total, used, buff_cache, zfs_arc]`.
    #[serde(default)]
    pub mem: Option<[u64; 4]>,
    /// Byte-accurate swap `[total, used]`.
    #[serde(default)]
    pub swap: Option<[u64; 2]>,
    /// Deprecated total memory in GiB (fallback when `mem` is absent).
    #[serde(default)]
    pub m: Option<f64>,
    /// Deprecated used memory in GiB (fallback when `mem` is absent).
    #[serde(default)]
    pub mu: Option<f64>,
    /// Deprecated buffer/cache memory in GiB (fallback when `mem` is absent).
    #[serde(default)]
    pub mb: Option<f64>,
}

impl RtMetricsPayload {
    /// Extract a normalized live CPU/memory sample stamped with `timestamp_ms`.
    pub fn to_live_sample(&self, timestamp_ms: i64) -> Option<LiveSample> {
        let stats = self.stats.as_ref()?;
        let cpu_pct = stats
            .cpu
            .or_else(|| self.info.as_ref().and_then(|i| i.cpu))?;
        let mem_pct = stats.mp.or_else(|| self.info.as_ref().and_then(|i| i.mp))?;

        let (mem_total_bytes, mem_used_bytes, mem_buff_cache_bytes) = stats.memory_bytes();
        let (swap_total_bytes, swap_used_bytes) = stats.swap_bytes();

        Some(LiveSample {
            timestamp_ms,
            cpu_pct,
            cpu_cores: stats.cpus.clone(),
            mem_pct,
            mem_used_bytes,
            mem_total_bytes,
            mem_buff_cache_bytes,
            swap_used_bytes,
            swap_total_bytes,
        })
    }
}

impl UpstreamStats {
    fn memory_bytes(&self) -> (Option<u64>, Option<u64>, Option<u64>) {
        if let Some([total, used, buff_cache, _zfs_arc]) = self.mem
            && total > 0
        {
            return (Some(total), Some(used), Some(buff_cache));
        }
        (
            self.m.and_then(gib_to_bytes),
            self.mu.and_then(gib_to_bytes),
            self.mb.and_then(gib_to_bytes),
        )
    }

    fn swap_bytes(&self) -> (Option<u64>, Option<u64>) {
        match self.swap {
            Some([total, used]) => (Some(total), Some(used)),
            None => (None, None),
        }
    }
}

fn gib_to_bytes(gib: f64) -> Option<u64> {
    if !gib.is_finite() || gib < 0.0 {
        return None;
    }
    Some((gib * GIB_BYTES).round() as u64)
}

/// Build the PocketBase realtime subscription topic string for a Beszel system ID.
pub fn rt_metrics_topic(system_id: &str) -> String {
    let options_json = serde_json::json!({
        "query": {
            "system": system_id,
        }
    })
    .to_string();
    format!("rt_metrics?options={}", percent_encode(&options_json))
}

fn percent_encode(input: &str) -> String {
    let mut encoded = String::with_capacity(input.len() * 2);
    for byte in input.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push('%');
            encoded.push(hex_nibble(byte >> 4));
            encoded.push(hex_nibble(byte & 0x0f));
        }
    }
    encoded
}

fn hex_nibble(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        10..=15 => (b'A' + (value - 10)) as char,
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct FixtureFile {
        event_topic_template: String,
        samples: Vec<RtMetricsPayload>,
    }

    #[test]
    fn builds_expected_rt_metrics_topic() {
        let topic = rt_metrics_topic("abc123456789012");
        assert_eq!(
            topic,
            "rt_metrics?options=%7B%22query%22%3A%7B%22system%22%3A%22abc123456789012%22%7D%7D"
        );
    }

    #[test]
    fn decodes_sanitized_live_fixture_samples() {
        let raw = include_str!("../../../docs/fixtures/rt_metrics_sample.json");
        let fixture: FixtureFile = serde_json::from_str(raw).expect("fixture should parse");
        assert_eq!(
            fixture.event_topic_template,
            "rt_metrics?options=%7B%22query%22%3A%7B%22system%22%3A%22<SYSTEM_ID>%22%7D%7D"
        );
        assert_eq!(fixture.samples.len(), 2);

        let first = fixture.samples[0]
            .to_live_sample(1_700_000_000_000)
            .expect("first sample should convert");
        assert_eq!(first.cpu_pct, 4.95);
        assert_eq!(
            first.cpu_cores.as_deref(),
            Some(&[0, 0, 0, 0, 0, 0, 8, 8, 8, 7, 8, 8, 9, 8, 8, 8][..])
        );
        assert_eq!(first.mem_pct, 13.45);
        assert_eq!(first.mem_total_bytes, Some(66_916_192_256));
        assert_eq!(first.mem_used_bytes, Some(8_998_420_480));
        assert_eq!(first.mem_buff_cache_bytes, Some(14_000_463_872));
        assert_eq!(first.swap_total_bytes, Some(8_589_930_496));
        assert_eq!(first.swap_used_bytes, Some(0));

        let second = fixture.samples[1]
            .to_live_sample(1_700_000_001_000)
            .expect("second sample should convert");
        assert_eq!(second.cpu_pct, 3.67);
        assert_eq!(second.mem_pct, 13.44);
    }

    #[test]
    fn falls_back_to_gib_memory_fields_when_mem_bytes_absent() {
        let payload: RtMetricsPayload =
            serde_json::from_str(r#"{"stats":{"cpu":12.5,"mp":50.0,"m":16.0,"mu":8.0,"mb":2.0}}"#)
                .expect("fallback payload should parse");

        let sample = payload
            .to_live_sample(42)
            .expect("fallback sample should convert");
        assert_eq!(sample.mem_total_bytes, Some(16 * 1024 * 1024 * 1024));
        assert_eq!(sample.mem_used_bytes, Some(8 * 1024 * 1024 * 1024));
        assert_eq!(sample.mem_buff_cache_bytes, Some(2 * 1024 * 1024 * 1024));
    }

    #[test]
    fn decodes_systems_collection_realtime_event() {
        let raw = r#"{"action":"update","record":{"id":"sys123456789012","name":"workstation","status":"up","info":{"v":"0.21.2"}}}"#;
        let event: SystemsRealtimeEvent =
            serde_json::from_str(raw).expect("systems event should parse");
        assert_eq!(event.action, "update");
        assert_eq!(event.record.id, "sys123456789012");
        assert_eq!(event.record.name, "workstation");
        assert_eq!(event.record.status, "up");
        assert_eq!(
            event.record.info.as_ref().and_then(|i| i.v.as_deref()),
            Some("0.21.2")
        );
    }
}
