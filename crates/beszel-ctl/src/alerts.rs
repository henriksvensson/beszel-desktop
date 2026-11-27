use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, IsTerminal, Write};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use beszel_protocol::upstream::{UpstreamAlertRecord, UpstreamSystemRecord};
use clap::{Args, Subcommand};
use serde::Serialize;

use crate::alerts_api::{BeszelAlertsClient, HubAccount, load_hub_account};
use crate::alerts_catalog::{
    AlertPreset, AlertSpec, ApplySummary, PlannedAlertBatch, RECOMMENDED_PRESETS, format_number,
    format_rule_condition, plan_preset_batches, resolve_alert_spec, select_target_systems,
    system_matches_requirement,
};

#[derive(Debug, Args)]
pub struct AlertsArgs {
    /// Optional path to beszel-desktop config.toml.
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: AlertsCommand,
}

#[derive(Debug, Subcommand)]
pub enum AlertsCommand {
    /// List configured and currently triggered Beszel alerts per host.
    List {
        /// Filter to a host name, system ID, comma-separated list, or `all`.
        #[arg(long)]
        system: Option<String>,
        /// Emit machine-readable JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Configure recommended Beszel alerts (interactive wizard, or --defaults for non-interactive).
    Setup {
        /// Enable opinionated recommended defaults non-interactively without overwriting existing rules.
        #[arg(long, alias = "yes", short = 'y')]
        defaults: bool,
        /// Target a host name, system ID, comma-separated list, or `all` (default: all hosts).
        #[arg(long)]
        system: Option<String>,
        /// Overwrite existing alert rules instead of preserving them.
        #[arg(long)]
        overwrite: bool,
        /// Emit machine-readable JSON summary (implies --defaults).
        #[arg(long)]
        json: bool,
    },
    /// Create or update one Beszel alert rule on one or all hosts.
    Set {
        /// Target host name, system ID, comma-separated list, or `all`.
        system: String,
        /// Beszel alert type (e.g. Disk, CPU, Memory, Temperature, Status, SystemdFailed, Battery).
        alert_type: String,
        /// Alert threshold value (omit for threshold-less alerts like Status or SystemdFailed).
        value: Option<f64>,
        /// Sustained duration in minutes before triggering.
        #[arg(long)]
        min: Option<u8>,
        /// Keep existing alert rules untouched if already configured on a target host.
        #[arg(long)]
        no_overwrite: bool,
    },
    /// Remove a Beszel alert rule from one or all hosts.
    Rm {
        /// Target host name, system ID, comma-separated list, or `all`.
        system: String,
        /// Beszel alert type to remove (e.g. Disk, CPU, Memory, Status, SystemdFailed).
        alert_type: String,
    },
}

#[derive(Debug, Serialize)]
struct HostAlertsJson<'a> {
    id: &'a str,
    name: &'a str,
    status: &'a str,
    alerts: Vec<AlertEntryJson<'a>>,
}

#[derive(Debug, Serialize)]
struct AlertEntryJson<'a> {
    id: &'a str,
    name: &'a str,
    value: Option<f64>,
    min: Option<u32>,
    triggered: bool,
}

struct InteractivePrompt {
    reader: BufReader<File>,
    out: File,
}

impl InteractivePrompt {
    fn open() -> io::Result<Self> {
        let reader = BufReader::new(OpenOptions::new().read(true).open("/dev/tty")?);
        let out = OpenOptions::new().write(true).open("/dev/tty")?;
        Ok(Self { reader, out })
    }

    fn ask(&mut self, prompt: &str) -> Result<String> {
        write!(self.out, "{prompt}")?;
        self.out.flush()?;
        let mut line = String::new();
        self.reader.read_line(&mut line)?;
        Ok(line.trim().to_string())
    }
}

pub async fn run_alerts(args: AlertsArgs) -> Result<()> {
    let account = load_hub_account(args.config.as_deref())?;
    let client = BeszelAlertsClient::connect(&account).await?;

    match args.command {
        AlertsCommand::List { system, json } => run_list(&client, system.as_deref(), json).await,
        AlertsCommand::Setup {
            defaults,
            system,
            overwrite,
            json,
        } => {
            let opts = SetupOptions {
                non_interactive: defaults || json,
                system,
                overwrite,
                json,
            };
            run_setup(&client, opts).await
        }
        AlertsCommand::Set {
            system,
            alert_type,
            value,
            min,
            no_overwrite,
        } => {
            let req = SetAlertRequest {
                system,
                alert_type,
                value,
                min,
                overwrite: !no_overwrite,
            };
            run_set(&client, req).await
        }
        AlertsCommand::Rm { system, alert_type } => run_rm(&client, &system, &alert_type).await,
    }
}

/// Optional post-`init` prompt offering to apply recommended Beszel alerts using the credentials
/// just entered (never overwrites existing alerts).
pub async fn offer_init_alerts_setup(account: &HubAccount) {
    let Ok(mut tty) = InteractivePrompt::open() else {
        return;
    };
    println!();
    let answer =
        tty.ask("Set up recommended Beszel alerts for your hosts (preserves existing)? [Y/n] ");
    let Ok(reply) = answer else {
        return;
    };
    if !is_affirmative_default_yes(&reply) {
        println!("Skipped alert setup (run `beszel-ctl alerts setup` anytime).");
        return;
    }

    match apply_defaults_for_account(account).await {
        Ok(summary) => print_apply_summary(&summary, false),
        Err(err) => println!("Note: could not configure Beszel alerts automatically: {err}"),
    }
}

async fn apply_defaults_for_account(account: &HubAccount) -> Result<ApplySummary> {
    let client = BeszelAlertsClient::connect(account).await?;
    let systems = client.fetch_systems().await?;
    let existing = client.fetch_alerts().await?;
    let targets: Vec<&UpstreamSystemRecord> = systems.iter().collect();
    let (batches, summary) = plan_preset_batches(RECOMMENDED_PRESETS, &targets, &existing, false);
    execute_batches(&client, &batches).await?;
    Ok(summary)
}

async fn run_list(client: &BeszelAlertsClient, system: Option<&str>, json: bool) -> Result<()> {
    let systems = client.fetch_systems().await?;
    let alerts = client.fetch_alerts().await?;
    let targets = select_target_systems(&systems, system)?;

    if json {
        let payload = build_list_json(&targets, &alerts);
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    print_alerts_table(&targets, &alerts);
    Ok(())
}

fn build_list_json<'a>(
    targets: &[&'a UpstreamSystemRecord],
    alerts: &'a [UpstreamAlertRecord],
) -> Vec<HostAlertsJson<'a>> {
    targets
        .iter()
        .map(|sys| {
            let mut host_alerts: Vec<AlertEntryJson<'a>> = alerts
                .iter()
                .filter(|a| a.system == sys.id)
                .map(|a| AlertEntryJson {
                    id: &a.id,
                    name: &a.name,
                    value: a.value,
                    min: a.min,
                    triggered: a.triggered,
                })
                .collect();
            host_alerts.sort_by_key(|a| a.name);
            HostAlertsJson {
                id: &sys.id,
                name: &sys.name,
                status: &sys.status,
                alerts: host_alerts,
            }
        })
        .collect()
}

fn print_alerts_table(targets: &[&UpstreamSystemRecord], alerts: &[UpstreamAlertRecord]) {
    let mut any_empty = false;
    for (idx, sys) in targets.iter().enumerate() {
        if idx > 0 {
            println!();
        }
        if print_host_alerts(sys, alerts) {
            any_empty = true;
        }
    }
    if any_empty {
        println!();
        println!("Tip: run `beszel-ctl alerts setup --defaults` to enable recommended defaults.");
    }
}

fn print_host_alerts(sys: &UpstreamSystemRecord, alerts: &[UpstreamAlertRecord]) -> bool {
    let mut host_alerts: Vec<&UpstreamAlertRecord> =
        alerts.iter().filter(|a| a.system == sys.id).collect();
    host_alerts.sort_by_key(|a| a.name.as_str());

    if host_alerts.is_empty() {
        println!("{} ({}) — no alerts configured", sys.name, sys.status);
        return true;
    }

    let triggered = host_alerts.iter().filter(|a| a.triggered).count();
    println!(
        "{} ({}) — {} configured, {} triggered",
        sys.name,
        sys.status,
        host_alerts.len(),
        triggered
    );
    for alert in host_alerts {
        let cond = format_rule_condition(&alert.name, alert.value, alert.min);
        let state = if alert.triggered { "TRIGGERED" } else { "ok" };
        println!("  {:<20} {:<26} {}", alert.name, cond, state);
    }
    false
}

struct SetupOptions {
    non_interactive: bool,
    system: Option<String>,
    overwrite: bool,
    json: bool,
}

async fn run_setup(client: &BeszelAlertsClient, opts: SetupOptions) -> Result<()> {
    let systems = client.fetch_systems().await?;
    let existing = client.fetch_alerts().await?;
    let targets = select_target_systems(&systems, opts.system.as_deref())?;

    let chosen_presets = if opts.non_interactive {
        RECOMMENDED_PRESETS.to_vec()
    } else {
        select_presets_interactively(&targets)?
    };
    if chosen_presets.is_empty() {
        println!("No alerts selected; nothing changed.");
        return Ok(());
    }

    let (batches, summary) =
        plan_preset_batches(&chosen_presets, &targets, &existing, opts.overwrite);
    execute_batches(client, &batches).await?;

    if opts.json {
        println!("{}", serde_json::to_string(&summary)?);
    } else {
        print_apply_summary(&summary, opts.overwrite);
    }
    Ok(())
}

fn select_presets_interactively(targets: &[&UpstreamSystemRecord]) -> Result<Vec<AlertPreset>> {
    let mut tty = open_interactive_tty()?;
    let host_names: Vec<&str> = targets.iter().map(|s| s.name.as_str()).collect();
    println!("Recommended Beszel alerts for: {}", host_names.join(", "));
    println!();
    for preset in RECOMMENDED_PRESETS {
        let count = targets
            .iter()
            .filter(|s| system_matches_requirement(s, preset.requirement))
            .count();
        if count > 0 {
            println!(
                "  {:<16} {:<32} ({count}/{} hosts)",
                preset.name,
                preset.summary,
                targets.len()
            );
        }
    }
    println!();

    let choice =
        tty.ask("Apply recommended defaults (preserves existing alerts)? [Y/n/c(ustomize)] ")?;
    if choice.eq_ignore_ascii_case("c") || choice.eq_ignore_ascii_case("customize") {
        return customize_presets(&mut tty, targets);
    }
    if is_affirmative_default_yes(&choice) {
        return Ok(RECOMMENDED_PRESETS.to_vec());
    }
    Ok(Vec::new())
}

fn open_interactive_tty() -> Result<InteractivePrompt> {
    if let Ok(tty) = InteractivePrompt::open() {
        return Ok(tty);
    }
    if !io::stdin().is_terminal() {
        bail!(
            "interactive terminal unavailable; pass `--defaults` to apply recommended alerts non-interactively"
        );
    }
    bail!(
        "unable to open /dev/tty; pass `--defaults` to apply recommended alerts non-interactively"
    )
}

fn customize_presets(
    tty: &mut InteractivePrompt,
    targets: &[&UpstreamSystemRecord],
) -> Result<Vec<AlertPreset>> {
    let mut selected = Vec::new();
    for &preset in RECOMMENDED_PRESETS {
        let applicable = targets
            .iter()
            .any(|s| system_matches_requirement(s, preset.requirement));
        if !applicable {
            continue;
        }
        if let Some(edited) = prompt_single_preset(tty, preset)? {
            selected.push(edited);
        }
    }
    Ok(selected)
}

fn prompt_single_preset(
    tty: &mut InteractivePrompt,
    mut preset: AlertPreset,
) -> Result<Option<AlertPreset>> {
    let spec = resolve_alert_spec(preset.name)?;
    let hint = format_preset_prompt(&preset, spec.has_threshold);
    let reply = tty.ask(&hint)?;

    if is_affirmative_default_yes(&reply) {
        return Ok(Some(preset));
    }
    if is_negative(&reply) {
        return Ok(None);
    }
    if let Some(custom) = parse_custom_threshold(&reply, spec.has_threshold) {
        preset.value = custom;
        return Ok(Some(preset));
    }
    println!(
        "  Skipping {} (unrecognized response {:?}).",
        preset.name, reply
    );
    Ok(None)
}

fn format_preset_prompt(preset: &AlertPreset, has_threshold: bool) -> String {
    if has_threshold {
        return format!(
            "Enable {} ({})? [Y/n or new threshold]: ",
            preset.name, preset.summary
        );
    }
    format!("Enable {} ({})? [Y/n]: ", preset.name, preset.summary)
}

fn parse_custom_threshold(reply: &str, has_threshold: bool) -> Option<f64> {
    if !has_threshold {
        return None;
    }
    let val = reply.parse::<f64>().ok()?;
    (val.is_finite() && val > 0.0).then_some(val)
}

fn is_negative(reply: &str) -> bool {
    let trimmed = reply.trim();
    trimmed.eq_ignore_ascii_case("n") || trimmed.eq_ignore_ascii_case("no")
}

async fn execute_batches(client: &BeszelAlertsClient, batches: &[PlannedAlertBatch]) -> Result<()> {
    for batch in batches {
        client.upsert_alerts(batch).await?;
    }
    Ok(())
}

fn print_apply_summary(summary: &ApplySummary, overwrite: bool) {
    let mode_note = if overwrite {
        "existing alerts updated"
    } else {
        "existing alerts preserved"
    };
    println!("Beszel alerts updated ({mode_note}):");
    for host in &summary.hosts {
        let mut parts = Vec::new();
        if !host.created.is_empty() {
            parts.push(format!(
                "enabled {} ({})",
                host.created.len(),
                host.created.join(", ")
            ));
        }
        if !host.updated.is_empty() {
            parts.push(format!(
                "updated {} ({})",
                host.updated.len(),
                host.updated.join(", ")
            ));
        }
        if !host.skipped.is_empty() {
            parts.push(format!("preserved {} existing", host.skipped.len()));
        }
        if parts.is_empty() {
            parts.push("no applicable presets".to_string());
        }
        println!("  {}: {}", host.name, parts.join("; "));
    }
}

struct SetAlertRequest {
    system: String,
    alert_type: String,
    value: Option<f64>,
    min: Option<u8>,
    overwrite: bool,
}

async fn run_set(client: &BeszelAlertsClient, req: SetAlertRequest) -> Result<()> {
    let spec = resolve_alert_spec(&req.alert_type)?;
    let value = validate_threshold(spec, req.value)?;
    let min = req.min.unwrap_or(spec.default_min);

    let systems = client.fetch_systems().await?;
    let targets = select_target_systems(&systems, Some(&req.system))?;
    let system_ids: Vec<String> = targets.iter().map(|s| s.id.clone()).collect();
    let host_names: Vec<&str> = targets.iter().map(|s| s.name.as_str()).collect();

    let batch = PlannedAlertBatch {
        name: spec.canonical.to_string(),
        value,
        min,
        system_ids,
        overwrite: req.overwrite,
    };
    client.upsert_alerts(&batch).await?;

    let cond = format_rule_condition(spec.canonical, Some(value), Some(u32::from(min)));
    println!(
        "Configured {} ({cond}) on: {}",
        spec.canonical,
        host_names.join(", ")
    );
    Ok(())
}

fn validate_threshold(spec: &AlertSpec, raw: Option<f64>) -> Result<f64> {
    if !spec.has_threshold {
        return Ok(raw.unwrap_or(0.0));
    }
    let value = raw.with_context(|| {
        format!(
            "{} alert requires a numeric threshold (e.g. `beszel-ctl alerts set all {} 85`)",
            spec.canonical, spec.canonical
        )
    })?;
    if !value.is_finite() || value <= 0.0 {
        bail!(
            "invalid threshold {} for {} alert; expected a positive number",
            format_number(value),
            spec.canonical
        );
    }
    Ok(value)
}

async fn run_rm(client: &BeszelAlertsClient, system: &str, alert_type: &str) -> Result<()> {
    let spec = resolve_alert_spec(alert_type)?;
    let systems = client.fetch_systems().await?;
    let targets = select_target_systems(&systems, Some(system))?;
    let system_ids: Vec<String> = targets.iter().map(|s| s.id.clone()).collect();
    let host_names: Vec<&str> = targets.iter().map(|s| s.name.as_str()).collect();

    let removed = client.delete_alerts(spec.canonical, &system_ids).await?;
    println!(
        "Removed {} {} alert rule(s) across: {}",
        removed,
        spec.canonical,
        host_names.join(", ")
    );
    Ok(())
}

fn is_affirmative_default_yes(reply: &str) -> bool {
    let trimmed = reply.trim();
    trimmed.is_empty() || trimmed.eq_ignore_ascii_case("y") || trimmed.eq_ignore_ascii_case("yes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_thresholds_for_numeric_and_boolean_alerts() {
        let disk = resolve_alert_spec("Disk").unwrap();
        assert_eq!(validate_threshold(disk, Some(85.0)).unwrap(), 85.0);
        assert!(validate_threshold(disk, None).is_err());
        assert!(validate_threshold(disk, Some(0.0)).is_err());
        assert!(validate_threshold(disk, Some(-5.0)).is_err());

        let status = resolve_alert_spec("Status").unwrap();
        assert_eq!(validate_threshold(status, None).unwrap(), 0.0);

        let systemd = resolve_alert_spec("SystemdFailed").unwrap();
        assert_eq!(validate_threshold(systemd, None).unwrap(), 0.0);
    }

    #[test]
    fn parses_affirmative_default_yes_replies() {
        assert!(is_affirmative_default_yes(""));
        assert!(is_affirmative_default_yes("y"));
        assert!(is_affirmative_default_yes("YES"));
        assert!(!is_affirmative_default_yes("n"));
        assert!(!is_affirmative_default_yes("no"));
    }
}
