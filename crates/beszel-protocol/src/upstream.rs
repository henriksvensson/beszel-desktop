//! Defensive wire types for Beszel / PocketBase HTTP and SSE payloads.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::local::{ContainerSample, GpuSample, LiveSample};

const GIB_BYTES: f64 = 1024.0 * 1024.0 * 1024.0;
const MIB_BYTES: f64 = 1024.0 * 1024.0;
const MAX_CONTAINERS: usize = 8;

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

/// PocketBase `GET /api/collections/system_details/records` paginated response.
#[derive(Debug, Clone, Deserialize)]
pub struct SystemDetailsListResponse {
    #[serde(default)]
    pub items: Vec<UpstreamSystemDetailsRecord>,
}

/// Static host metadata stored in Beszel's `system_details` collection.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct UpstreamSystemDetailsRecord {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub system: String,
    #[serde(default)]
    pub os: Option<u8>,
    #[serde(default)]
    pub os_name: Option<String>,
    #[serde(default)]
    pub kernel: Option<String>,
    #[serde(default)]
    pub cpu: Option<String>,
    #[serde(default)]
    pub arch: Option<String>,
}

impl UpstreamSystemDetailsRecord {
    /// System ID key used to join with `UpstreamSystemRecord`.
    pub fn system_key(&self) -> &str {
        if !self.system.is_empty() {
            &self.system
        } else {
            &self.id
        }
    }
}

/// Map Beszel's numeric OS enum (`0=Linux, 1=Darwin, 2=Windows, 3=FreeBSD`) to a stable label.
pub fn os_family_label(code: u8) -> Option<&'static str> {
    match code {
        0 => Some("linux"),
        1 => Some("macos"),
        2 => Some("windows"),
        3 => Some("freebsd"),
        _ => None,
    }
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
    #[serde(default)]
    pub container: Vec<UpstreamContainerStats>,
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
    /// Root disk percent summary.
    #[serde(default)]
    pub dp: Option<f64>,
    /// Dashboard / peak temperature in Celsius.
    #[serde(default)]
    pub dt: Option<f64>,
    /// Load averages `[1m, 5m, 15m]`.
    #[serde(default)]
    pub la: Option<[f64; 3]>,
    /// Battery `[percent, state]`.
    #[serde(default)]
    pub bat: Option<[u8; 2]>,
    /// Legacy OS enum (`0=Linux, 1=Darwin, 2=Windows, 3=FreeBSD`) for hubs without `system_details`.
    #[serde(default)]
    pub os: Option<u8>,
    /// Legacy kernel version for hubs without `system_details`.
    #[serde(default)]
    pub k: Option<String>,
    /// Legacy CPU model for hubs without `system_details`.
    #[serde(default)]
    pub m: Option<String>,
}

/// Upstream per-GPU metrics map entry (`stats.g`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct UpstreamGpuData {
    #[serde(default)]
    pub n: String,
    #[serde(default)]
    pub u: f64,
    #[serde(default)]
    pub mu: Option<f64>,
    #[serde(default)]
    pub mt: Option<f64>,
    #[serde(default)]
    pub p: Option<f64>,
}

/// Upstream storage pool health entry (`stats.z`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct UpstreamPoolStats {
    #[serde(default)]
    pub n: Option<String>,
    #[serde(default)]
    pub h: Option<String>,
}

/// Upstream container metrics entry (`rt_metrics.container[]`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct UpstreamContainerStats {
    #[serde(default)]
    pub n: String,
    #[serde(default)]
    pub c: f64,
    #[serde(default)]
    pub m: f64,
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
    /// Root disk total GiB.
    #[serde(default)]
    pub d: Option<f64>,
    /// Root disk used GiB.
    #[serde(default)]
    pub du: Option<f64>,
    /// Root disk utilization percent.
    #[serde(default)]
    pub dp: Option<f64>,
    /// Aggregate disk I/O `[read_bytes/s, write_bytes/s]`.
    #[serde(default)]
    pub dio: Option<[u64; 2]>,
    /// Aggregate bandwidth `[sent_bytes/s, recv_bytes/s]`.
    #[serde(default)]
    pub b: Option<[u64; 2]>,
    /// Deprecated network sent MB/s fallback.
    #[serde(default)]
    pub ns: Option<f64>,
    /// Deprecated network recv MB/s fallback.
    #[serde(default)]
    pub nr: Option<f64>,
    /// Load averages `[1m, 5m, 15m]`.
    #[serde(default)]
    pub la: Option<[f64; 3]>,
    /// Temperature sensors map (`name -> °C`).
    #[serde(default)]
    pub t: Option<BTreeMap<String, f64>>,
    /// Battery `[percent, state]`.
    #[serde(default)]
    pub bat: Option<[u8; 2]>,
    /// Per-GPU data map (`id -> GPUData`).
    #[serde(default)]
    pub g: Option<BTreeMap<String, UpstreamGpuData>>,
    /// Storage pools map (`id -> pool stats`).
    #[serde(default)]
    pub z: Option<BTreeMap<String, UpstreamPoolStats>>,
}

impl RtMetricsPayload {
    /// Extract a normalized live sample stamped with `timestamp_ms`.
    pub fn to_live_sample(&self, timestamp_ms: i64) -> Option<LiveSample> {
        let stats = self.stats.as_ref()?;
        let info = self.info.as_ref();
        let cpu_pct = stats.cpu.or_else(|| info.and_then(|i| i.cpu))?;
        let mem_pct = stats.mp.or_else(|| info.and_then(|i| i.mp))?;

        let (mem_total_bytes, mem_used_bytes, mem_buff_cache_bytes) = stats.memory_bytes();
        let (swap_total_bytes, swap_used_bytes) = stats.swap_bytes();
        let (net_sent_bps, net_recv_bps) = stats.network_bps();
        let (disk_read_bps, disk_write_bps) = stats.disk_io_bps();

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
            uptime_secs: info.and_then(|i| i.u),
            load_avg: stats.la.or_else(|| info.and_then(|i| i.la)),
            temp_max_c: stats.peak_temp_c(info),
            battery_pct: stats.battery_pct(info),
            net_sent_bps,
            net_recv_bps,
            disk_pct: stats.dp.or_else(|| info.and_then(|i| i.dp)),
            disk_used_gb: stats.du,
            disk_total_gb: stats.d,
            disk_read_bps,
            disk_write_bps,
            unhealthy_pools: stats.unhealthy_pools(),
            gpus: stats.gpu_samples(),
            containers: extract_top_containers(&self.container),
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

    fn network_bps(&self) -> (Option<u64>, Option<u64>) {
        if let Some([sent, recv]) = self.b {
            return (Some(sent), Some(recv));
        }
        (
            self.ns.and_then(mib_to_bytes),
            self.nr.and_then(mib_to_bytes),
        )
    }

    fn disk_io_bps(&self) -> (Option<u64>, Option<u64>) {
        match self.dio {
            Some([read, write]) => (Some(read), Some(write)),
            None => (None, None),
        }
    }

    fn peak_temp_c(&self, info: Option<&UpstreamSystemInfo>) -> Option<f64> {
        if let Some(dt) = info.and_then(|i| i.dt)
            && dt > 0.0
        {
            return Some(dt);
        }
        let temps = self.t.as_ref()?;
        temps
            .values()
            .copied()
            .filter(|v| v.is_finite() && *v > 0.0)
            .reduce(f64::max)
    }

    fn battery_pct(&self, info: Option<&UpstreamSystemInfo>) -> Option<u8> {
        let bat = self.bat.or_else(|| info.and_then(|i| i.bat))?;
        Some(bat[0].min(100))
    }

    fn unhealthy_pools(&self) -> Option<Vec<String>> {
        let pools = self.z.as_ref()?;
        let degraded: Vec<String> = pools
            .iter()
            .filter_map(|(key, pool)| {
                let health = pool.h.as_deref()?;
                if health.eq_ignore_ascii_case("ONLINE") {
                    return None;
                }
                let name = pool.n.as_deref().unwrap_or(key);
                Some(format!("{name}: {health}"))
            })
            .collect();
        (!degraded.is_empty()).then_some(degraded)
    }

    fn gpu_samples(&self) -> Option<Vec<GpuSample>> {
        let map = self.g.as_ref()?;
        if map.is_empty() {
            return None;
        }
        let samples = map
            .iter()
            .map(|(id, gpu)| GpuSample {
                name: if gpu.n.is_empty() {
                    id.clone()
                } else {
                    gpu.n.clone()
                },
                usage_pct: gpu.u,
                mem_used_mb: gpu.mu.filter(|v| *v >= 0.0),
                mem_total_mb: gpu.mt.filter(|v| *v > 0.0),
                power_w: gpu.p.filter(|v| *v > 0.0),
            })
            .collect();
        Some(samples)
    }
}

fn extract_top_containers(raw: &[UpstreamContainerStats]) -> Option<Vec<ContainerSample>> {
    if raw.is_empty() {
        return None;
    }
    let mut list: Vec<ContainerSample> = raw
        .iter()
        .filter(|c| !c.n.is_empty())
        .map(|c| ContainerSample {
            name: c.n.clone(),
            cpu_pct: c.c,
            mem_mb: c.m,
        })
        .collect();
    if list.is_empty() {
        return None;
    }
    list.sort_by(|a, b| {
        b.cpu_pct
            .total_cmp(&a.cpu_pct)
            .then_with(|| b.mem_mb.total_cmp(&a.mem_mb))
    });
    list.truncate(MAX_CONTAINERS);
    Some(list)
}

fn gib_to_bytes(gib: f64) -> Option<u64> {
    if !gib.is_finite() || gib < 0.0 {
        return None;
    }
    Some((gib * GIB_BYTES).round() as u64)
}

fn mib_to_bytes(mib: f64) -> Option<u64> {
    if !mib.is_finite() || mib < 0.0 {
        return None;
    }
    Some((mib * MIB_BYTES).round() as u64)
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
        assert_eq!(first.uptime_secs, Some(17_752));
        assert_eq!(first.load_avg, Some([0.81, 0.76, 0.87]));
        assert_eq!(first.temp_max_c, Some(44.75));
        assert_eq!(first.battery_pct, Some(100));
        assert_eq!(first.net_sent_bps, Some(9_001));
        assert_eq!(first.net_recv_bps, Some(92_367));
        assert_eq!(first.disk_pct, Some(6.94));
        assert_eq!(first.disk_read_bps, Some(115_725));
        assert_eq!(first.disk_write_bps, Some(3_857_418));
        assert_eq!(first.unhealthy_pools, None);

        let second = fixture.samples[1]
            .to_live_sample(1_700_000_001_000)
            .expect("second sample should convert");
        assert_eq!(second.cpu_pct, 3.67);
        assert_eq!(second.mem_pct, 13.44);
        assert_eq!(second.net_sent_bps, Some(2_155));
        assert_eq!(second.net_recv_bps, Some(844));
    }

    #[test]
    fn decodes_gpu_containers_and_degraded_pools() {
        let raw = r#"{
            "stats": {
                "cpu": 25.0,
                "mp": 40.0,
                "g": {
                    "0": {"n": "RTX 4060", "u": 62.5, "mu": 3072.0, "mt": 8192.0, "p": 88.4}
                },
                "z": {
                    "pool1": {"n": "tank", "h": "DEGRADED"}
                }
            },
            "container": [
                {"n": "db", "c": 1.2, "m": 512.0},
                {"n": "worker", "c": 18.5, "m": 256.0}
            ]
        }"#;
        let payload: RtMetricsPayload = serde_json::from_str(raw).expect("parse");
        let sample = payload.to_live_sample(99).expect("convert");

        let gpus = sample.gpus.expect("gpu present");
        assert_eq!(gpus.len(), 1);
        assert_eq!(gpus[0].name, "RTX 4060");
        assert_eq!(gpus[0].usage_pct, 62.5);
        assert_eq!(gpus[0].power_w, Some(88.4));

        let containers = sample.containers.expect("containers present");
        assert_eq!(containers[0].name, "worker");
        assert_eq!(containers[1].name, "db");

        assert_eq!(
            sample.unhealthy_pools,
            Some(vec!["tank: DEGRADED".to_string()])
        );
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
