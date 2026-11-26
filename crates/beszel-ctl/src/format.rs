use std::fmt::Write as _;

use beszel_protocol::local::{ConnectionState, ConnectionStatus, LiveSample, SystemSummary};
use serde::Serialize;

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

/// Local client view of the relay state used to render shell-specific outputs.
#[derive(Debug, Default)]
pub struct ClientView {
    pub connection: Option<ConnectionStatus>,
    pub systems: Vec<SystemSummary>,
    pub latest_samples: Vec<(String, LiveSample)>,
}

impl ClientView {
    pub fn apply_snapshot(
        &mut self,
        connection: ConnectionStatus,
        systems: Vec<(SystemSummary, Option<LiveSample>)>,
    ) {
        self.connection = Some(connection);
        self.systems = systems.iter().map(|(s, _)| s.clone()).collect();
        self.latest_samples = systems
            .into_iter()
            .filter_map(|(s, sample)| sample.map(|val| (s.id, val)))
            .collect();
    }

    pub fn update_sample(&mut self, system_id: &str, sample: LiveSample) {
        if let Some(conn) = &mut self.connection {
            conn.last_sample_ms = Some(sample.timestamp_ms);
        }
        if let Some((_, existing)) = self
            .latest_samples
            .iter_mut()
            .find(|(id, _)| id == system_id)
        {
            *existing = sample;
        } else {
            self.latest_samples.push((system_id.to_string(), sample));
        }
    }

    pub fn select_system(
        &self,
        filter: Option<&str>,
    ) -> Option<(&SystemSummary, Option<&LiveSample>)> {
        let system = match filter {
            Some(target) => self
                .systems
                .iter()
                .find(|s| s.id == target || s.name.eq_ignore_ascii_case(target))?,
            None => self.systems.first()?,
        };
        let sample = self
            .latest_samples
            .iter()
            .find(|(id, _)| id == &system.id)
            .map(|(_, s)| s);
        Some((system, sample))
    }
}

/// Waybar custom module JSON payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WaybarOutput {
    pub text: String,
    pub tooltip: String,
    pub class: String,
    pub percentage: u8,
}

/// Severity thresholds (percent) for Waybar CSS class selection.
#[derive(Debug, Clone, Copy)]
pub struct WaybarOptions<'a> {
    pub system_filter: Option<&'a str>,
    pub stale_after_ms: i64,
    pub warn_pct: f64,
    pub danger_pct: f64,
}

/// Render a Waybar JSON object representing the current relay state and staleness.
pub fn format_waybar(view: &ClientView, opts: WaybarOptions<'_>, now_ms: i64) -> WaybarOutput {
    let Some(conn) = view.connection.as_ref() else {
        return WaybarOutput {
            text: "Beszel: offline".into(),
            tooltip: "Cannot connect to beszel-relay socket".into(),
            class: "disconnected".into(),
            percentage: 0,
        };
    };

    let Some((system, sample)) = view.select_system(opts.system_filter) else {
        return WaybarOutput {
            text: format!("Beszel: {}", state_label(conn.state)),
            tooltip: format_connection_tooltip(conn),
            class: state_class(conn.state).into(),
            percentage: 0,
        };
    };

    let Some(sample) = sample else {
        return WaybarOutput {
            text: format!("{}: waiting", system.name),
            tooltip: format_connection_tooltip(conn),
            class: state_class(conn.state).into(),
            percentage: 0,
        };
    };

    let age_ms = now_ms.saturating_sub(sample.timestamp_ms);
    let stale_secs = (age_ms > opts.stale_after_ms).then_some(age_ms / 1000);
    let class = waybar_class(conn.state, sample, stale_secs.is_some(), opts);

    let text = format!("CPU {:>2.0}% MEM {:>2.0}%", sample.cpu_pct, sample.mem_pct);
    let tooltip = build_waybar_tooltip(system, sample, conn, stale_secs);
    let percentage = sample.cpu_pct.clamp(0.0, 100.0).round() as u8;

    WaybarOutput {
        text,
        tooltip,
        class: class.into(),
        percentage,
    }
}

fn waybar_class(
    state: ConnectionState,
    sample: &LiveSample,
    is_stale: bool,
    opts: WaybarOptions<'_>,
) -> &'static str {
    if state != ConnectionState::Connected {
        return state_class(state);
    }
    if is_stale {
        return "stale";
    }
    let peak = sample.cpu_pct.max(sample.mem_pct);
    if peak >= opts.danger_pct {
        "danger"
    } else if peak >= opts.warn_pct {
        "warning"
    } else {
        "connected"
    }
}

fn build_waybar_tooltip(
    system: &SystemSummary,
    sample: &LiveSample,
    conn: &ConnectionStatus,
    stale_secs: Option<i64>,
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{} ({})", system.name, state_label(conn.state));
    let _ = writeln!(out, "CPU: {:.1}%", sample.cpu_pct);
    let _ = write!(out, "Memory: {:.1}%", sample.mem_pct);
    if let (Some(used), Some(total)) = (sample.mem_used_bytes, sample.mem_total_bytes) {
        let _ = write!(
            out,
            " ({:.2} / {:.2} GiB)",
            used as f64 / GIB,
            total as f64 / GIB
        );
    }
    if let Some(secs) = stale_secs {
        let _ = write!(out, "\nStale: {secs}s since last sample");
    }
    if let Some(err) = &conn.last_error {
        let _ = write!(out, "\nLast error: {err}");
    }
    out
}

fn format_connection_tooltip(conn: &ConnectionStatus) -> String {
    match &conn.last_error {
        Some(err) => format!("State: {}\nLast error: {err}", state_label(conn.state)),
        None => format!("State: {}", state_label(conn.state)),
    }
}

/// Format a concise human-readable line for `beszel-ctl watch`.
pub fn format_watch_sample(system_label: &str, sample: &LiveSample) -> String {
    match (sample.mem_used_bytes, sample.mem_total_bytes) {
        (Some(used), Some(total)) => format!(
            "[{}] {}  cpu={:5.1}%  mem={:5.1}% ({:.2}/{:.2} GiB)",
            sample.timestamp_ms,
            system_label,
            sample.cpu_pct,
            sample.mem_pct,
            used as f64 / GIB,
            total as f64 / GIB,
        ),
        _ => format!(
            "[{}] {}  cpu={:5.1}%  mem={:5.1}%",
            sample.timestamp_ms, system_label, sample.cpu_pct, sample.mem_pct
        ),
    }
}

pub fn state_label(state: ConnectionState) -> &'static str {
    state_meta(state).0
}

fn state_class(state: ConnectionState) -> &'static str {
    state_meta(state).1
}

fn state_meta(state: ConnectionState) -> (&'static str, &'static str) {
    match state {
        ConnectionState::Connecting => ("connecting", "connecting"),
        ConnectionState::Connected => ("connected", "connected"),
        ConnectionState::Paused => ("paused", "paused"),
        ConnectionState::Reconnecting => ("reconnecting", "reconnecting"),
        ConnectionState::Disconnected => ("disconnected", "disconnected"),
        ConnectionState::AuthError => ("auth error", "error"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(ts: i64, cpu_pct: f64, mem_pct: f64) -> LiveSample {
        LiveSample {
            timestamp_ms: ts,
            cpu_pct,
            mem_pct,
            mem_used_bytes: Some(8 * 1024 * 1024 * 1024),
            mem_total_bytes: Some(16 * 1024 * 1024 * 1024),
            ..LiveSample::default()
        }
    }

    fn default_opts() -> WaybarOptions<'static> {
        WaybarOptions {
            system_filter: None,
            stale_after_ms: 5000,
            warn_pct: 70.0,
            danger_pct: 90.0,
        }
    }

    #[test]
    fn formats_connected_severity_and_stale_waybar_states() {
        let mut view = ClientView::default();
        view.apply_snapshot(
            ConnectionStatus {
                state: ConnectionState::Connected,
                since_ms: 1000,
                last_sample_ms: Some(5000),
                last_error: None,
                reconnect_attempts: 0,
            },
            vec![(
                SystemSummary {
                    id: "sys1".into(),
                    name: "workstation".into(),
                    status: "up".into(),
                    agent_version: Some("0.21.2".into()),
                    ..SystemSummary::default()
                },
                Some(sample(5000, 12.4, 48.6)),
            )],
        );

        let fresh = format_waybar(&view, default_opts(), 6000);
        assert_eq!(fresh.class, "connected");
        assert_eq!(fresh.text, "CPU 12% MEM 49%");
        assert_eq!(fresh.percentage, 12);
        assert!(fresh.tooltip.contains("8.00 / 16.00 GiB"));

        let stale = format_waybar(&view, default_opts(), 15_000);
        assert_eq!(stale.class, "stale");
        assert!(stale.tooltip.contains("Stale: 10s"));

        view.update_sample("sys1", sample(16_000, 74.0, 50.0));
        assert_eq!(
            format_waybar(&view, default_opts(), 17_000).class,
            "warning"
        );

        view.update_sample("sys1", sample(18_000, 94.0, 50.0));
        assert_eq!(format_waybar(&view, default_opts(), 19_000).class, "danger");
    }

    #[test]
    fn formats_offline_waybar_state_without_relay() {
        let view = ClientView::default();
        let out = format_waybar(&view, default_opts(), 10_000);
        assert_eq!(out.class, "disconnected");
        assert_eq!(out.text, "Beszel: offline");
    }
}
