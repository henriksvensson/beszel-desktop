use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use beszel_protocol::default_socket_path;
use serde::Deserialize;

pub const DEFAULT_HISTORY_CAPACITY: usize = 60;
pub const SYSTEMD_PASSWORD_CREDENTIAL: &str = "beszel-password";

/// Runtime configuration for `beszel-relay`.
#[derive(Clone)]
pub struct RelayConfig {
    pub hub_url: String,
    pub email: String,
    pub password: String,
    pub system: Option<String>,
    pub socket_path: PathBuf,
    pub history_capacity: usize,
}

impl fmt::Debug for RelayConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RelayConfig")
            .field("hub_url", &self.hub_url)
            .field("email", &"<redacted>")
            .field("password", &"<redacted>")
            .field("system", &self.system)
            .field("socket_path", &self.socket_path)
            .field("history_capacity", &self.history_capacity)
            .finish()
    }
}

#[derive(Debug, Default, Deserialize)]
struct FileConfig {
    hub_url: Option<String>,
    email: Option<String>,
    password: Option<String>,
    password_file: Option<PathBuf>,
    system: Option<String>,
    socket_path: Option<PathBuf>,
    history_capacity: Option<usize>,
}

/// CLI overrides merged on top of environment variables and TOML config.
#[derive(Debug, Default)]
pub struct CliOverrides {
    pub config_path: Option<PathBuf>,
    pub system: Option<String>,
    pub socket_path: Option<PathBuf>,
    pub history_capacity: Option<usize>,
}

impl RelayConfig {
    /// Load configuration with precedence: CLI flags > env vars > systemd credentials > config file.
    pub fn load(cli: CliOverrides) -> Result<Self> {
        let file = load_file_config(cli.config_path.as_deref())?;
        Self::merge(file, cli, |key| std::env::var(key).ok())
    }

    fn merge<F>(file: FileConfig, cli: CliOverrides, env_get: F) -> Result<Self>
    where
        F: Fn(&str) -> Option<String>,
    {
        let password = resolve_password(&file, &env_get)?;

        let hub_url = pick_non_empty(env_get("BESZEL_HUB_URL"), file.hub_url)
            .context("missing hub URL (set BESZEL_HUB_URL or hub_url in config.toml)")?
            .trim_end_matches('/')
            .to_string();

        let email = pick_non_empty(env_get("BESZEL_EMAIL"), file.email)
            .context("missing account email (set BESZEL_EMAIL or email in config.toml)")?;

        let system = cli
            .system
            .filter(|s| !s.is_empty())
            .or_else(|| pick_non_empty(env_get("BESZEL_SYSTEM"), file.system));

        let socket_path = cli
            .socket_path
            .or_else(|| {
                env_get("BESZEL_SOCKET_PATH")
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from)
            })
            .or(file.socket_path)
            .unwrap_or_else(default_socket_path);

        let history_capacity = resolve_history_capacity(
            cli.history_capacity,
            env_get("BESZEL_HISTORY_CAPACITY"),
            file.history_capacity,
        )?;

        Ok(Self {
            hub_url,
            email,
            password,
            system,
            socket_path,
            history_capacity,
        })
    }
}

fn resolve_password<F>(file: &FileConfig, env_get: &F) -> Result<String>
where
    F: Fn(&str) -> Option<String>,
{
    if let Some(password) = env_get("BESZEL_PASSWORD")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        return Ok(password);
    }

    if let Some(path) = env_get("BESZEL_PASSWORD_FILE")
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
    {
        return read_secret_file(&path);
    }

    if let Some(cred_path) = systemd_credential_path(env_get)
        && cred_path.exists()
    {
        return read_secret_file(&cred_path);
    }

    if let Some(path) = &file.password_file {
        return read_secret_file(&expand_tilde(path, env_get));
    }

    pick_non_empty(None, file.password.clone()).context(
        "missing account password (use systemd credential 'beszel-password', BESZEL_PASSWORD_FILE, BESZEL_PASSWORD, or password_file in config.toml)",
    )
}

fn systemd_credential_path<F>(env_get: &F) -> Option<PathBuf>
where
    F: Fn(&str) -> Option<String>,
{
    env_get("CREDENTIALS_DIRECTORY")
        .filter(|dir| !dir.trim().is_empty())
        .map(|dir| PathBuf::from(dir).join(SYSTEMD_PASSWORD_CREDENTIAL))
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

fn expand_tilde<F>(path: &Path, env_get: &F) -> PathBuf
where
    F: Fn(&str) -> Option<String>,
{
    let Ok(rest) = path.strip_prefix("~") else {
        return path.to_path_buf();
    };
    match env_get("HOME").filter(|h| !h.is_empty()) {
        Some(home) => PathBuf::from(home).join(rest),
        None => path.to_path_buf(),
    }
}

fn pick_non_empty(primary: Option<String>, fallback: Option<String>) -> Option<String> {
    primary
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            fallback
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
}

fn resolve_history_capacity(
    cli: Option<usize>,
    env: Option<String>,
    file: Option<usize>,
) -> Result<usize> {
    let env_val = match env.filter(|s| !s.trim().is_empty()) {
        Some(raw) => Some(
            raw.trim()
                .parse::<usize>()
                .with_context(|| format!("invalid BESZEL_HISTORY_CAPACITY: {raw}"))?,
        ),
        None => None,
    };
    let capacity = cli.or(env_val).or(file).unwrap_or(DEFAULT_HISTORY_CAPACITY);
    if capacity == 0 {
        bail!("history_capacity must be at least 1");
    }
    Ok(capacity)
}

fn load_file_config(explicit_path: Option<&Path>) -> Result<FileConfig> {
    if let Some(path) = explicit_path {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config file {}", path.display()))?;
        return toml::from_str(&raw)
            .with_context(|| format!("failed to parse config file {}", path.display()));
    }
    let Some(default_path) = default_config_path() else {
        return Ok(FileConfig::default());
    };
    if !default_path.exists() {
        return Ok(FileConfig::default());
    }
    let raw = std::fs::read_to_string(&default_path)
        .with_context(|| format!("failed to read config file {}", default_path.display()))?;
    toml::from_str(&raw)
        .with_context(|| format!("failed to parse config file {}", default_path.display()))
}

fn default_config_path() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        return Some(PathBuf::from(xdg).join("beszel-desktop/config.toml"));
    }
    std::env::var("HOME")
        .ok()
        .filter(|h| !h.is_empty())
        .map(|home| PathBuf::from(home).join(".config/beszel-desktop/config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_overrides_file_and_cli_overrides_env() {
        let file = FileConfig {
            hub_url: Some("https://file.example.com/".into()),
            email: Some("file@example.com".into()),
            password: Some("file-pass".into()),
            password_file: None,
            system: Some("file-sys".into()),
            socket_path: Some(PathBuf::from("/tmp/file.sock")),
            history_capacity: Some(30),
        };
        let cli = CliOverrides {
            config_path: None,
            system: Some("cli-sys".into()),
            socket_path: None,
            history_capacity: Some(90),
        };
        let cfg = RelayConfig::merge(file, cli, |key| match key {
            "BESZEL_HUB_URL" => Some("https://env.example.com/".into()),
            "BESZEL_SOCKET_PATH" => Some("/tmp/env.sock".into()),
            _ => None,
        })
        .expect("merge should succeed");

        assert_eq!(cfg.hub_url, "https://env.example.com");
        assert_eq!(cfg.email, "file@example.com");
        assert_eq!(cfg.system.as_deref(), Some("cli-sys"));
        assert_eq!(cfg.socket_path, PathBuf::from("/tmp/env.sock"));
        assert_eq!(cfg.history_capacity, 90);
    }

    #[test]
    fn loads_password_from_systemd_credentials_directory() {
        let dir = std::env::temp_dir().join(format!("beszel-cred-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp cred dir");
        std::fs::write(dir.join(SYSTEMD_PASSWORD_CREDENTIAL), "cred-secret\n")
            .expect("write cred file");

        let file = FileConfig {
            hub_url: Some("https://hub.example.com".into()),
            email: Some("user@example.com".into()),
            ..Default::default()
        };
        let dir_str = dir.to_string_lossy().to_string();
        let cfg = RelayConfig::merge(file, CliOverrides::default(), |key| match key {
            "CREDENTIALS_DIRECTORY" => Some(dir_str.clone()),
            _ => None,
        })
        .expect("merge should load systemd credential");

        assert_eq!(cfg.password, "cred-secret");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn debug_redacts_credentials() {
        let cfg = RelayConfig {
            hub_url: "https://hub.example.com".into(),
            email: "secret@example.com".into(),
            password: "super-secret-password".into(),
            system: None,
            socket_path: PathBuf::from("/tmp/test.sock"),
            history_capacity: 60,
        };
        let debug = format!("{cfg:?}");
        assert!(!debug.contains("secret@example.com"));
        assert!(!debug.contains("super-secret-password"));
    }
}
