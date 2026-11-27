use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use beszel_protocol::upstream::{
    AlertsListResponse, AuthResponse, SystemsListResponse, UpstreamAlertRecord,
    UpstreamSystemRecord,
};
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::alerts_catalog::PlannedAlertBatch;

const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const SYSTEMD_PASSWORD_CREDENTIAL: &str = "beszel-password";

/// Account credentials used by `beszel-ctl alerts` to call Beszel Hub REST APIs.
#[derive(Clone)]
pub struct HubAccount {
    pub hub_url: String,
    pub email: String,
    pub password: String,
}

impl fmt::Debug for HubAccount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HubAccount")
            .field("hub_url", &self.hub_url)
            .field("email", &"<redacted>")
            .field("password", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Default, Deserialize)]
struct StoredFileConfig {
    hub_url: Option<String>,
    email: Option<String>,
    password: Option<String>,
    password_file: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
struct UpsertAlertsRequest<'a> {
    name: &'a str,
    value: f64,
    min: u8,
    systems: &'a [String],
    overwrite: bool,
}

#[derive(Debug, Serialize)]
struct DeleteAlertsRequest<'a> {
    name: &'a str,
    systems: &'a [String],
}

#[derive(Debug, Deserialize)]
struct DeleteAlertsResponse {
    #[serde(default)]
    count: u16,
}

/// Authenticated Beszel Hub REST client for managing alert rules.
pub struct BeszelAlertsClient {
    client: Client,
    hub_url: String,
    token: String,
}

impl BeszelAlertsClient {
    pub async fn connect(account: &HubAccount) -> Result<Self> {
        let client = Client::builder()
            .timeout(HTTP_TIMEOUT)
            .build()
            .context("failed to build HTTP client")?;
        let token = authenticate(&client, account).await?;
        Ok(Self {
            client,
            hub_url: account.hub_url.trim_end_matches('/').to_string(),
            token,
        })
    }

    async fn get_json<T: DeserializeOwned>(&self, path: &str, resource: &str) -> Result<T> {
        let url = format!("{}{}", self.hub_url, path);
        let resp = self
            .client
            .get(&url)
            .header("Authorization", &self.token)
            .send()
            .await
            .with_context(|| format!("failed to fetch {resource} from Beszel hub"))?;
        if !resp.status().is_success() {
            bail!("listing {resource} failed (HTTP {})", resp.status());
        }
        resp.json::<T>()
            .await
            .with_context(|| format!("invalid {resource} response"))
    }

    pub async fn fetch_systems(&self) -> Result<Vec<UpstreamSystemRecord>> {
        let mut list: SystemsListResponse = self
            .get_json("/api/collections/systems/records?perPage=200", "systems")
            .await?;
        list.items.sort_by_key(|s| s.name.to_lowercase());
        Ok(list.items)
    }

    pub async fn fetch_alerts(&self) -> Result<Vec<UpstreamAlertRecord>> {
        let list: AlertsListResponse = self
            .get_json(
                "/api/collections/alerts/records?perPage=500&fields=id,name,system,value,min,triggered",
                "alerts",
            )
            .await?;
        Ok(list.items)
    }

    pub async fn upsert_alerts(&self, batch: &PlannedAlertBatch) -> Result<()> {
        if batch.system_ids.is_empty() {
            return Ok(());
        }
        let url = format!("{}/api/beszel/user-alerts", self.hub_url);
        let body = UpsertAlertsRequest {
            name: &batch.name,
            value: batch.value,
            min: batch.min,
            systems: &batch.system_ids,
            overwrite: batch.overwrite,
        };
        let resp = self
            .client
            .post(&url)
            .header("Authorization", &self.token)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("failed to upsert {} alert on Beszel hub", batch.name))?;
        if !resp.status().is_success() {
            bail!(
                "Beszel hub rejected {} alert update (HTTP {})",
                batch.name,
                resp.status()
            );
        }
        Ok(())
    }

    pub async fn delete_alerts(&self, name: &str, system_ids: &[String]) -> Result<u16> {
        if system_ids.is_empty() {
            return Ok(0);
        }
        let url = format!("{}/api/beszel/user-alerts", self.hub_url);
        let body = DeleteAlertsRequest {
            name,
            systems: system_ids,
        };
        let resp = self
            .client
            .delete(&url)
            .header("Authorization", &self.token)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("failed to delete {name} alert on Beszel hub"))?;
        if !resp.status().is_success() {
            bail!(
                "Beszel hub rejected {name} alert deletion (HTTP {})",
                resp.status()
            );
        }
        let parsed: DeleteAlertsResponse = resp
            .json()
            .await
            .unwrap_or(DeleteAlertsResponse { count: 0 });
        Ok(parsed.count)
    }
}

async fn authenticate(client: &Client, account: &HubAccount) -> Result<String> {
    let url = format!(
        "{}/api/collections/users/auth-with-password",
        account.hub_url.trim_end_matches('/')
    );
    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "identity": account.email,
            "password": account.password,
        }))
        .send()
        .await
        .with_context(|| format!("failed to reach Beszel hub at {}", account.hub_url))?;
    if !resp.status().is_success() {
        bail!(
            "Beszel hub rejected credentials (HTTP {}); run `beszel-ctl init` to refresh them",
            resp.status()
        );
    }
    let auth: AuthResponse = resp
        .json()
        .await
        .context("unexpected auth response from Beszel hub")?;
    Ok(auth.token)
}

/// Resolve Beszel hub URL, email, and password from env vars, config.toml, or systemd-creds.
pub fn load_hub_account(explicit_config: Option<&Path>) -> Result<HubAccount> {
    let config_path = resolve_config_path(explicit_config);
    let file_cfg = read_stored_config(config_path.as_deref())?;
    let config_dir = config_path
        .as_deref()
        .and_then(Path::parent)
        .map(Path::to_path_buf);

    let password = resolve_account_password(&file_cfg, config_dir.as_deref())?;
    let hub_url = pick_non_empty(env_opt("BESZEL_HUB_URL"), file_cfg.hub_url)
        .context("missing Beszel hub URL (run `beszel-ctl init` or set BESZEL_HUB_URL)")?
        .trim_end_matches('/')
        .to_string();
    let email = pick_non_empty(env_opt("BESZEL_EMAIL"), file_cfg.email)
        .context("missing Beszel email (run `beszel-ctl init` or set BESZEL_EMAIL)")?;

    Ok(HubAccount {
        hub_url,
        email,
        password,
    })
}

fn resolve_account_password(
    file_cfg: &StoredFileConfig,
    config_dir: Option<&Path>,
) -> Result<String> {
    if let Some(pw) = env_opt("BESZEL_PASSWORD") {
        return Ok(pw);
    }
    if let Some(path) = env_opt("BESZEL_PASSWORD_FILE") {
        return read_secret_file(Path::new(&path));
    }
    if let Some(cred_dir) = env_opt("CREDENTIALS_DIRECTORY") {
        let cred_path = PathBuf::from(cred_dir).join(SYSTEMD_PASSWORD_CREDENTIAL);
        if cred_path.is_file() {
            return read_secret_file(&cred_path);
        }
    }
    if let Some(path) = &file_cfg.password_file {
        return read_secret_file(&expand_tilde(path));
    }
    if let Some(pw) = pick_non_empty(None, file_cfg.password.clone()) {
        return Ok(pw);
    }
    if let Some(dir) = config_dir {
        return load_stored_credential(dir);
    }
    bail!("missing Beszel password (run `beszel-ctl init` or set BESZEL_PASSWORD)")
}

fn load_stored_credential(config_dir: &Path) -> Result<String> {
    let encrypted = config_dir.join("beszel-password.cred");
    if encrypted.is_file() {
        return decrypt_systemd_cred(&encrypted);
    }
    let plain = config_dir.join("beszel-password");
    if plain.is_file() {
        return read_secret_file(&plain);
    }
    bail!(
        "no Beszel password credential found in {} (run `beszel-ctl init` or set BESZEL_PASSWORD)",
        config_dir.display()
    )
}

fn decrypt_systemd_cred(path: &Path) -> Result<String> {
    let output = Command::new("systemd-creds")
        .args([
            "decrypt",
            "--user",
            &format!("--name={SYSTEMD_PASSWORD_CREDENTIAL}"),
        ])
        .arg(path)
        .arg("-")
        .output()
        .context("failed to run `systemd-creds decrypt`")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("failed to decrypt {}: {}", path.display(), stderr.trim());
    }
    let raw =
        String::from_utf8(output.stdout).context("decrypted credential is not valid UTF-8")?;
    let trimmed = raw.trim_end_matches(['\r', '\n']);
    if trimmed.is_empty() {
        bail!("decrypted credential {} is empty", path.display());
    }
    Ok(trimmed.to_string())
}

fn resolve_config_path(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = explicit {
        return Some(path.to_path_buf());
    }
    if let Some(xdg) = env_opt("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(xdg).join("beszel-desktop/config.toml"));
    }
    env_opt("HOME").map(|h| PathBuf::from(h).join(".config/beszel-desktop/config.toml"))
}

fn read_stored_config(path: Option<&Path>) -> Result<StoredFileConfig> {
    let Some(path) = path else {
        return Ok(StoredFileConfig::default());
    };
    if !path.is_file() {
        return Ok(StoredFileConfig::default());
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))
}

fn read_secret_file(path: &Path) -> Result<String> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read password file {}", path.display()))?;
    let trimmed = raw.trim_end_matches(['\r', '\n']);
    if trimmed.is_empty() {
        bail!("password file {} is empty", path.display());
    }
    Ok(trimmed.to_string())
}

fn expand_tilde(path: &Path) -> PathBuf {
    let Ok(rest) = path.strip_prefix("~") else {
        return path.to_path_buf();
    };
    match env_opt("HOME") {
        Some(home) => PathBuf::from(home).join(rest),
        None => path.to_path_buf(),
    }
}

fn env_opt(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn pick_non_empty(primary: Option<String>, fallback: Option<String>) -> Option<String> {
    primary.or_else(|| {
        fallback
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_hub_account() {
        let account = HubAccount {
            hub_url: "https://hub.example.test".into(),
            email: "alice@example.test".into(),
            password: "secret-password".into(),
        };
        let rendered = format!("{account:?}");
        assert!(!rendered.contains("alice@example.test"));
        assert!(!rendered.contains("secret-password"));
    }
}
