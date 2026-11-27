use std::collections::{BTreeMap, HashSet};

use anyhow::{Context, Result, bail};
use beszel_protocol::upstream::{UpstreamAlertRecord, UpstreamSystemInfo, UpstreamSystemRecord};
use serde::Serialize;

/// Static metadata for a canonical Beszel alert type.
#[derive(Debug, Clone, Copy)]
pub struct AlertSpec {
    pub canonical: &'static str,
    pub label: &'static str,
    pub op: &'static str,
    pub unit: &'static str,
    pub has_threshold: bool,
    pub default_min: u8,
}

/// Host capability predicate controlling whether an opinionated preset applies to a system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostRequirement {
    Any,
    Systemd,
    Temperature,
    Battery,
}

/// One recommended Beszel alert rule in the opinionated preset set.
#[derive(Debug, Clone, Copy)]
pub struct AlertPreset {
    pub name: &'static str,
    pub value: f64,
    pub min: u8,
    pub requirement: HostRequirement,
    pub summary: &'static str,
}

/// Planned non-destructive (or overwrite) alert write for a single alert type across hosts.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedAlertBatch {
    pub name: String,
    pub value: f64,
    pub min: u8,
    pub system_ids: Vec<String>,
    pub overwrite: bool,
}

/// Per-host accounting of created, updated, and preserved (skipped) alert rules.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct HostApplySummary {
    pub id: String,
    pub name: String,
    pub created: Vec<String>,
    pub updated: Vec<String>,
    pub skipped: Vec<String>,
}

/// Aggregate result of applying a set of alert presets.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct ApplySummary {
    pub created: usize,
    pub updated: usize,
    pub skipped: usize,
    pub hosts: Vec<HostApplySummary>,
}

pub const ALERT_SPECS: &[AlertSpec] = &[
    AlertSpec {
        canonical: "Status",
        label: "Host down",
        op: "",
        unit: "",
        has_threshold: false,
        default_min: 2,
    },
    AlertSpec {
        canonical: "CPU",
        label: "CPU",
        op: ">=",
        unit: "%",
        has_threshold: true,
        default_min: 10,
    },
    AlertSpec {
        canonical: "CPUIOWait",
        label: "CPU I/O wait",
        op: ">=",
        unit: "%",
        has_threshold: true,
        default_min: 5,
    },
    AlertSpec {
        canonical: "CPUSteal",
        label: "CPU steal",
        op: ">=",
        unit: "%",
        has_threshold: true,
        default_min: 5,
    },
    AlertSpec {
        canonical: "Memory",
        label: "Memory",
        op: ">=",
        unit: "%",
        has_threshold: true,
        default_min: 5,
    },
    AlertSpec {
        canonical: "Disk",
        label: "Disk",
        op: ">=",
        unit: "%",
        has_threshold: true,
        default_min: 1,
    },
    AlertSpec {
        canonical: "Bandwidth",
        label: "Bandwidth",
        op: ">=",
        unit: " MB/s",
        has_threshold: true,
        default_min: 5,
    },
    AlertSpec {
        canonical: "GPU",
        label: "GPU",
        op: ">=",
        unit: "%",
        has_threshold: true,
        default_min: 5,
    },
    AlertSpec {
        canonical: "Temperature",
        label: "Temperature",
        op: ">=",
        unit: "°C",
        has_threshold: true,
        default_min: 5,
    },
    AlertSpec {
        canonical: "Battery",
        label: "Battery",
        op: "<=",
        unit: "%",
        has_threshold: true,
        default_min: 1,
    },
    AlertSpec {
        canonical: "LoadAvg1",
        label: "Load (1m)",
        op: ">=",
        unit: "",
        has_threshold: true,
        default_min: 5,
    },
    AlertSpec {
        canonical: "LoadAvg5",
        label: "Load (5m)",
        op: ">=",
        unit: "",
        has_threshold: true,
        default_min: 5,
    },
    AlertSpec {
        canonical: "LoadAvg15",
        label: "Load (15m)",
        op: ">=",
        unit: "",
        has_threshold: true,
        default_min: 5,
    },
    AlertSpec {
        canonical: "ContainerHealth",
        label: "Unhealthy container",
        op: "",
        unit: "",
        has_threshold: false,
        default_min: 1,
    },
    AlertSpec {
        canonical: "SystemdFailed",
        label: "Failed systemd service",
        op: "",
        unit: "",
        has_threshold: false,
        default_min: 0,
    },
    AlertSpec {
        canonical: "NetworkMonitorLoss",
        label: "Packet loss",
        op: ">=",
        unit: "%",
        has_threshold: true,
        default_min: 0,
    },
    AlertSpec {
        canonical: "NetworkMonitorCert",
        label: "TLS cert expiring",
        op: "<=",
        unit: "d",
        has_threshold: true,
        default_min: 0,
    },
];

pub const RECOMMENDED_PRESETS: &[AlertPreset] = &[
    AlertPreset {
        name: "Status",
        value: 0.0,
        min: 2,
        requirement: HostRequirement::Any,
        summary: "Host unreachable for 2m",
    },
    AlertPreset {
        name: "Disk",
        value: 85.0,
        min: 1,
        requirement: HostRequirement::Any,
        summary: "Any disk >= 85% for 1m",
    },
    AlertPreset {
        name: "SystemdFailed",
        value: 0.0,
        min: 0,
        requirement: HostRequirement::Systemd,
        summary: "Any failed systemd unit",
    },
    AlertPreset {
        name: "Temperature",
        value: 85.0,
        min: 5,
        requirement: HostRequirement::Temperature,
        summary: "Any sensor >= 85°C for 5m",
    },
    AlertPreset {
        name: "CPU",
        value: 90.0,
        min: 10,
        requirement: HostRequirement::Any,
        summary: "Sustained CPU >= 90% for 10m",
    },
    AlertPreset {
        name: "Memory",
        value: 90.0,
        min: 5,
        requirement: HostRequirement::Any,
        summary: "Sustained memory >= 90% for 5m",
    },
    AlertPreset {
        name: "Battery",
        value: 15.0,
        min: 1,
        requirement: HostRequirement::Battery,
        summary: "Battery <= 15% for 1m",
    },
];

/// Look up canonical alert metadata by exact name or user-friendly alias.
pub fn resolve_alert_spec(raw: &str) -> Result<&'static AlertSpec> {
    let key = normalize_alert_key(raw);
    let canonical = match key.as_str() {
        "status" | "down" | "offline" => "Status",
        "cpu" => "CPU",
        "cpuiowait" | "iowait" => "CPUIOWait",
        "cpusteal" | "steal" => "CPUSteal",
        "memory" | "mem" | "ram" => "Memory",
        "disk" | "storage" => "Disk",
        "bandwidth" | "net" | "network" => "Bandwidth",
        "gpu" => "GPU",
        "temperature" | "temp" => "Temperature",
        "battery" | "bat" => "Battery",
        "loadavg1" | "load1" => "LoadAvg1",
        "loadavg5" | "load5" => "LoadAvg5",
        "loadavg15" | "load15" => "LoadAvg15",
        "containerhealth" | "container" | "containers" => "ContainerHealth",
        "systemdfailed" | "systemd" | "services" | "failedservices" => "SystemdFailed",
        "networkmonitorloss" | "loss" | "packetloss" => "NetworkMonitorLoss",
        "networkmonitorcert" | "cert" | "tls" => "NetworkMonitorCert",
        _ => {
            let valid: Vec<&str> = ALERT_SPECS.iter().map(|s| s.canonical).collect();
            bail!(
                "unknown alert type {:?}; valid types: {}",
                raw,
                valid.join(", ")
            );
        }
    };
    find_alert_spec(canonical).context("internal alert spec lookup failed")
}

/// Find alert spec by exact canonical name (for display formatting).
pub fn find_alert_spec(canonical: &str) -> Option<&'static AlertSpec> {
    ALERT_SPECS.iter().find(|spec| spec.canonical == canonical)
}

fn normalize_alert_key(raw: &str) -> String {
    raw.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Resolve a `--system` filter (ID, case-insensitive name, comma-separated list, or `all`).
pub fn select_target_systems<'a>(
    systems: &'a [UpstreamSystemRecord],
    filter: Option<&str>,
) -> Result<Vec<&'a UpstreamSystemRecord>> {
    if systems.is_empty() {
        bail!("no systems found on this Beszel hub");
    }
    let Some(raw) = filter.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(systems.iter().collect());
    };
    if raw.eq_ignore_ascii_case("all") {
        return Ok(systems.iter().collect());
    }

    let mut matched = Vec::new();
    let mut seen_ids = HashSet::new();
    for token in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let found = find_matching_system(systems, token)?;
        if seen_ids.insert(found.id.as_str()) {
            matched.push(found);
        }
    }
    if matched.is_empty() {
        bail!("no systems matched {:?}", raw);
    }
    Ok(matched)
}

fn find_matching_system<'a>(
    systems: &'a [UpstreamSystemRecord],
    token: &str,
) -> Result<&'a UpstreamSystemRecord> {
    systems
        .iter()
        .find(|s| s.id == token || s.name.eq_ignore_ascii_case(token))
        .with_context(|| {
            let names: Vec<&str> = systems.iter().map(|s| s.name.as_str()).collect();
            format!(
                "system {:?} not found; available: {}",
                token,
                names.join(", ")
            )
        })
}

/// Check whether a system supports a given preset requirement.
pub fn system_matches_requirement(
    system: &UpstreamSystemRecord,
    requirement: HostRequirement,
) -> bool {
    let info = system.info.as_ref();
    match requirement {
        HostRequirement::Any => true,
        HostRequirement::Battery => info.and_then(|i| i.bat).is_some(),
        HostRequirement::Temperature => info.and_then(|i| i.dt).is_some_and(|t| t > 0.0),
        HostRequirement::Systemd => is_systemd_host(info),
    }
}

fn is_systemd_host(info: Option<&UpstreamSystemInfo>) -> bool {
    let Some(info) = info else {
        return false;
    };
    if info.os.is_some_and(|code| code != 0) {
        return false;
    }
    if info.sv.as_ref().is_some_and(|sv| !sv.is_empty()) {
        return true;
    }
    info.os == Some(0)
}

/// Build the API write batches and per-host summary for a slice of presets without overwriting
/// existing alert rules unless `overwrite` is explicitly requested.
pub fn plan_preset_batches(
    presets: &[AlertPreset],
    targets: &[&UpstreamSystemRecord],
    existing: &[UpstreamAlertRecord],
    overwrite: bool,
) -> (Vec<PlannedAlertBatch>, ApplySummary) {
    let existing_keys: HashSet<(&str, &str)> = existing
        .iter()
        .map(|a| (a.system.as_str(), a.name.as_str()))
        .collect();
    let ctx = PlanContext {
        existing: &existing_keys,
        overwrite,
    };
    let mut host_map = init_host_summary_map(targets);
    let mut batches = Vec::new();

    for preset in presets {
        let system_ids = apply_preset_to_hosts(preset, targets, &mut host_map, &ctx);
        if !system_ids.is_empty() {
            batches.push(PlannedAlertBatch {
                name: preset.name.to_string(),
                value: preset.value,
                min: preset.min,
                system_ids,
                overwrite,
            });
        }
    }

    (batches, finalize_apply_summary(targets, host_map))
}

struct PlanContext<'a> {
    existing: &'a HashSet<(&'a str, &'a str)>,
    overwrite: bool,
}

fn apply_preset_to_hosts(
    preset: &AlertPreset,
    targets: &[&UpstreamSystemRecord],
    host_map: &mut BTreeMap<String, HostApplySummary>,
    ctx: &PlanContext<'_>,
) -> Vec<String> {
    let mut system_ids = Vec::new();
    for sys in targets {
        if !system_matches_requirement(sys, preset.requirement) {
            continue;
        }
        let already_exists = ctx.existing.contains(&(sys.id.as_str(), preset.name));
        let entry = host_map.get_mut(&sys.id).expect("target host registered");
        record_host_decision(entry, preset.name, already_exists, ctx.overwrite);
        if !already_exists || ctx.overwrite {
            system_ids.push(sys.id.clone());
        }
    }
    system_ids
}

fn init_host_summary_map(targets: &[&UpstreamSystemRecord]) -> BTreeMap<String, HostApplySummary> {
    targets
        .iter()
        .map(|sys| {
            (
                sys.id.clone(),
                HostApplySummary {
                    id: sys.id.clone(),
                    name: sys.name.clone(),
                    ..HostApplySummary::default()
                },
            )
        })
        .collect()
}

fn record_host_decision(
    entry: &mut HostApplySummary,
    alert_name: &str,
    already_exists: bool,
    overwrite: bool,
) {
    if !already_exists {
        entry.created.push(alert_name.to_string());
    } else if overwrite {
        entry.updated.push(alert_name.to_string());
    } else {
        entry.skipped.push(alert_name.to_string());
    }
}

fn finalize_apply_summary(
    targets: &[&UpstreamSystemRecord],
    mut host_map: BTreeMap<String, HostApplySummary>,
) -> ApplySummary {
    let mut summary = ApplySummary::default();
    for sys in targets {
        if let Some(host) = host_map.remove(&sys.id) {
            summary.created += host.created.len();
            summary.updated += host.updated.len();
            summary.skipped += host.skipped.len();
            summary.hosts.push(host);
        }
    }
    summary
}

/// Format an alert threshold + duration cleanly for terminal output.
pub fn format_rule_condition(name: &str, value: Option<f64>, min: Option<u32>) -> String {
    let base = match find_alert_spec(name) {
        Some(spec) if !spec.has_threshold => spec.label.to_string(),
        Some(spec) => {
            let num = format_number(value.unwrap_or(0.0));
            format!("{} {}{}", spec.op, num, spec.unit)
        }
        None => match value {
            Some(v) => format_number(v),
            None => "-".to_string(),
        },
    };
    match min.filter(|&m| m > 0) {
        Some(m) => format!("{base} ({m}m)"),
        None => base,
    }
}

pub fn format_number(val: f64) -> String {
    if (val - val.round()).abs() < 1e-6 {
        format!("{:.0}", val.round())
    } else {
        format!("{val:.1}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_system(
        id: &str,
        name: &str,
        info: Option<UpstreamSystemInfo>,
    ) -> UpstreamSystemRecord {
        UpstreamSystemRecord {
            id: id.into(),
            name: name.into(),
            status: "up".into(),
            info,
        }
    }

    #[test]
    fn normalizes_alert_names_and_aliases() {
        assert_eq!(resolve_alert_spec("disk").unwrap().canonical, "Disk");
        assert_eq!(resolve_alert_spec("MEM").unwrap().canonical, "Memory");
        assert_eq!(resolve_alert_spec("temp").unwrap().canonical, "Temperature");
        assert_eq!(
            resolve_alert_spec("systemd").unwrap().canonical,
            "SystemdFailed"
        );
        assert_eq!(
            resolve_alert_spec("container-health").unwrap().canonical,
            "ContainerHealth"
        );
        assert_eq!(resolve_alert_spec("load5").unwrap().canonical, "LoadAvg5");
        assert!(resolve_alert_spec("bogus_alert").is_err());
    }

    #[test]
    fn filters_presets_by_host_capabilities_and_preserves_existing() {
        let laptop = sample_system(
            "sys_laptop",
            "laptop",
            Some(UpstreamSystemInfo {
                dt: Some(52.0),
                bat: Some([80, 4]),
                sv: Some(vec![40, 0]),
                ..UpstreamSystemInfo::default()
            }),
        );
        let server = sample_system(
            "sys_server",
            "server",
            Some(UpstreamSystemInfo {
                os: Some(0),
                ..UpstreamSystemInfo::default()
            }),
        );
        let systems = [laptop, server];
        let targets: Vec<&UpstreamSystemRecord> = systems.iter().collect();

        let existing = vec![UpstreamAlertRecord {
            id: "a1".into(),
            system: "sys_laptop".into(),
            name: "Disk".into(),
            triggered: false,
            value: Some(92.0),
            min: Some(5),
        }];

        let (batches, summary) =
            plan_preset_batches(RECOMMENDED_PRESETS, &targets, &existing, false);

        assert_eq!(summary.skipped, 1);
        assert_eq!(summary.hosts[0].skipped, vec!["Disk"]);
        assert!(summary.hosts[0].created.contains(&"Battery".to_string()));
        assert!(
            summary.hosts[0]
                .created
                .contains(&"Temperature".to_string())
        );
        assert!(!summary.hosts[1].created.contains(&"Battery".to_string()));
        assert!(
            !summary.hosts[1]
                .created
                .contains(&"Temperature".to_string())
        );
        assert!(
            summary.hosts[1]
                .created
                .contains(&"SystemdFailed".to_string())
        );

        let disk_batch = batches
            .iter()
            .find(|b| b.name == "Disk")
            .expect("disk batch");
        assert_eq!(disk_batch.system_ids, vec!["sys_server"]);
        assert!(!disk_batch.overwrite);
    }

    #[test]
    fn selects_systems_by_name_id_or_all() {
        let systems = vec![
            sample_system("id_1", "alpha", None),
            sample_system("id_2", "beta", None),
        ];
        assert_eq!(select_target_systems(&systems, None).unwrap().len(), 2);
        assert_eq!(
            select_target_systems(&systems, Some("all")).unwrap().len(),
            2
        );
        let one = select_target_systems(&systems, Some("ALPHA")).unwrap();
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].id, "id_1");
        let both = select_target_systems(&systems, Some("id_2,alpha,alpha")).unwrap();
        assert_eq!(both.len(), 2);
        assert!(select_target_systems(&systems, Some("missing")).is_err());
    }
}
