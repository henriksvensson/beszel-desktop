use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use beszel_protocol::upstream::UpstreamSystemRecord;
use clap::Args;
use serde::{Deserialize, Serialize};
use tokio::net::UnixStream;

#[derive(Debug, Args)]
pub struct InitArgs {
    /// Beszel hub base URL (e.g. https://beszel.example.com).
    #[arg(long)]
    pub hub_url: Option<String>,
    /// Beszel account email.
    #[arg(long)]
    pub email: Option<String>,
    /// Optional comma-separated host filter (omit to stream all online hosts).
    #[arg(long)]
    pub system: Option<String>,
    /// Skip live authentication check against the Beszel hub.
    #[arg(long)]
    pub no_verify: bool,
    /// Write config and credentials without installing or starting the systemd user service.
    #[arg(long)]
    pub no_service: bool,
    /// Store password in a 0600 file (LoadCredential=) instead of systemd-creds encryption.
    #[arg(long)]
    pub plaintext_password_file: bool,
}

#[derive(Debug, Clone)]
struct HubCredentials {
    hub_url: String,
    email: String,
    password: String,
    system: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CredentialMode {
    Encrypted(PathBuf),
    PlainFile(PathBuf),
}

#[derive(Debug, Serialize)]
struct StoredConfig<'a> {
    hub_url: &'a str,
    email: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<&'a str>,
    history_capacity: usize,
}

#[derive(Debug, Deserialize)]
struct AuthResponse {
    token: String,
}

#[derive(Debug, Deserialize)]
struct SystemsListResponse {
    #[serde(default)]
    items: Vec<UpstreamSystemRecord>,
}

struct TtyPrompt {
    reader: BufReader<File>,
    out: File,
}

struct EchoGuard;

impl Drop for EchoGuard {
    fn drop(&mut self) {
        let _ = set_tty_echo(true);
    }
}

pub async fn run_init(args: InitArgs, socket_path: &Path) -> Result<()> {
    println!("Beszel Desktop setup");
    println!("--------------------");

    let home = resolve_home_dir()?;
    let creds = collect_credentials(&args)?;

    if !args.no_verify {
        verify_hub_credentials(&creds).await?;
    }

    let config_dir = home.join(".config").join("beszel-desktop");
    ensure_private_dir(&config_dir)?;

    let cred_mode = store_password(&config_dir, &creds.password, args.plaintext_password_file)?;
    let config_path = write_config_toml(&config_dir, &creds)?;
    println!("Wrote config: {}", config_path.display());

    if !args.no_service {
        let unit_path = install_and_start_service(&home, &cred_mode)?;
        println!("Started systemd user service: {}", unit_path.display());
        wait_for_socket(socket_path).await;
    }

    println!();
    println!("Setup complete! Your desktop widget should connect automatically.");
    Ok(())
}

fn resolve_home_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME environment variable is not set")?;
    if home.trim().is_empty() {
        bail!("HOME environment variable is empty");
    }
    Ok(PathBuf::from(home))
}

fn collect_credentials(args: &InitArgs) -> Result<HubCredentials> {
    let mut tty = TtyPrompt::open().ok();

    let raw_url = match &args.hub_url {
        Some(url) => url.clone(),
        None => prompt_line(
            &mut tty,
            "Beszel hub URL (e.g. https://beszel.example.com): ",
        )?,
    };
    let hub_url = normalize_hub_url(&raw_url)?;

    let email = match &args.email {
        Some(val) => val.trim().to_string(),
        None => prompt_line(&mut tty, "Beszel account email: ")?,
    };
    if email.is_empty() {
        bail!("Beszel account email cannot be empty");
    }

    let password = prompt_password(&mut tty, "Beszel account password: ")?;
    if password.is_empty() {
        bail!("Beszel account password cannot be empty");
    }

    let system = args
        .system
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    Ok(HubCredentials {
        hub_url,
        email,
        password,
        system,
    })
}

impl TtyPrompt {
    fn open() -> io::Result<Self> {
        let read_file = OpenOptions::new().read(true).open("/dev/tty")?;
        let out = OpenOptions::new().write(true).open("/dev/tty")?;
        Ok(Self {
            reader: BufReader::new(read_file),
            out,
        })
    }

    fn read_line(&mut self, label: &str) -> Result<String> {
        write!(self.out, "{label}")?;
        self.out.flush()?;
        let mut line = String::new();
        self.reader.read_line(&mut line)?;
        Ok(line.trim().to_string())
    }
}

fn prompt_line(tty: &mut Option<TtyPrompt>, label: &str) -> Result<String> {
    if let Some(prompt) = tty.as_mut() {
        return prompt.read_line(label);
    }
    print!("{label}");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

fn prompt_password(tty: &mut Option<TtyPrompt>, label: &str) -> Result<String> {
    if let Some(prompt) = tty.as_mut() {
        let _guard = disable_tty_echo();
        let value = prompt.read_line(label)?;
        writeln!(prompt.out)?;
        prompt.out.flush()?;
        return Ok(value);
    }
    prompt_line(tty, label)
}

fn disable_tty_echo() -> Option<EchoGuard> {
    if set_tty_echo(false).is_ok() {
        Some(EchoGuard)
    } else {
        None
    }
}

fn set_tty_echo(enabled: bool) -> io::Result<()> {
    let tty = OpenOptions::new().read(true).open("/dev/tty")?;
    let flag = if enabled { "echo" } else { "-echo" };
    let status = Command::new("stty")
        .arg(flag)
        .stdin(Stdio::from(tty))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other("stty failed"))
    }
}

fn normalize_hub_url(raw: &str) -> Result<String> {
    let trimmed = raw.trim().trim_end_matches('/');
    if !trimmed.starts_with("http://") && !trimmed.starts_with("https://") {
        bail!("hub URL must start with http:// or https://");
    }
    if trimmed == "http://" || trimmed == "https://" {
        bail!("hub URL must include a host name");
    }
    Ok(trimmed.to_string())
}

async fn verify_hub_credentials(creds: &HubCredentials) -> Result<()> {
    println!("Verifying login against {} ...", creds.hub_url);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .context("failed to build HTTP client")?;

    let auth_url = format!("{}/api/collections/users/auth-with-password", creds.hub_url);
    let resp = client
        .post(&auth_url)
        .json(&serde_json::json!({
            "identity": creds.email,
            "password": creds.password,
        }))
        .send()
        .await
        .with_context(|| format!("failed to reach Beszel hub at {}", creds.hub_url))?;

    if !resp.status().is_success() {
        bail!(
            "hub rejected credentials (HTTP {}); check URL, email, and password",
            resp.status()
        );
    }

    let auth: AuthResponse = resp
        .json()
        .await
        .context("unexpected auth response from Beszel hub")?;
    print_discovered_hosts(&client, &creds.hub_url, &auth.token).await
}

async fn print_discovered_hosts(
    client: &reqwest::Client,
    hub_url: &str,
    token: &str,
) -> Result<()> {
    let url = format!("{hub_url}/api/collections/systems/records?perPage=50");
    let resp = client
        .get(&url)
        .header("Authorization", token)
        .send()
        .await
        .context("authenticated, but failed to query Beszel systems")?;

    if !resp.status().is_success() {
        bail!(
            "authenticated, but listing systems failed (HTTP {})",
            resp.status()
        );
    }

    let list: SystemsListResponse = resp.json().await.context("invalid systems list response")?;
    if list.items.is_empty() {
        println!("Authenticated! No systems found yet on this hub.");
        return Ok(());
    }

    let summary: Vec<String> = list
        .items
        .iter()
        .map(|s| format!("{} ({})", s.name, s.status))
        .collect();
    println!(
        "Authenticated! Found {} host(s): {}",
        list.items.len(),
        summary.join(", ")
    );
    Ok(())
}

fn ensure_private_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir)
        .with_context(|| format!("failed to create directory {}", dir.display()))?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("failed to chmod 700 {}", dir.display()))?;
    Ok(())
}

fn write_private_file(path: &Path, contents: &[u8]) -> Result<()> {
    if path.exists() {
        fs::remove_file(path).with_context(|| format!("failed to replace {}", path.display()))?;
    }
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("failed to create {}", path.display()))?;
    file.write_all(contents)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

fn store_password(config_dir: &Path, password: &str, force_plain: bool) -> Result<CredentialMode> {
    if !force_plain {
        let cred_path = config_dir.join("beszel-password.cred");
        if try_encrypt_with_systemd_creds(&cred_path, password)? {
            println!("Stored encrypted credential via systemd-creds (TPM2/host key).");
            return Ok(CredentialMode::Encrypted(cred_path));
        }
    }

    let plain_path = config_dir.join("beszel-password");
    write_private_file(&plain_path, password.as_bytes())?;
    println!(
        "Stored user-only password file (chmod 600) at {} (systemd-creds --user unavailable).",
        plain_path.display()
    );
    Ok(CredentialMode::PlainFile(plain_path))
}

fn try_encrypt_with_systemd_creds(dest: &Path, password: &str) -> Result<bool> {
    let mut child = match Command::new("systemd-creds")
        .args(["encrypt", "--user", "--name=beszel-password", "-"])
        .arg(dest)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return Ok(false),
    };

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(password.as_bytes());
    }

    let status = child.wait().context("failed waiting for systemd-creds")?;
    if !status.success() {
        let _ = fs::remove_file(dest);
        return Ok(false);
    }

    fs::set_permissions(dest, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("failed to chmod 600 {}", dest.display()))?;
    Ok(true)
}

fn write_config_toml(config_dir: &Path, creds: &HubCredentials) -> Result<PathBuf> {
    let path = config_dir.join("config.toml");
    let body = render_config_toml(creds)?;
    write_private_file(&path, body.as_bytes())?;
    Ok(path)
}

fn render_config_toml(creds: &HubCredentials) -> Result<String> {
    let stored = StoredConfig {
        hub_url: &creds.hub_url,
        email: &creds.email,
        system: creds.system.as_deref(),
        history_capacity: 60,
    };
    let serialized = toml::to_string_pretty(&stored).context("failed to serialize config.toml")?;
    Ok(format!(
        "# Generated by beszel-ctl init.\n# Password is supplied via systemd LoadCredential(Encrypted).\n\n{serialized}"
    ))
}

fn resolve_relay_binary(home: &Path) -> PathBuf {
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let sibling = dir.join("beszel-relay");
        if sibling.is_file() {
            return sibling;
        }
    }
    home.join(".local").join("bin").join("beszel-relay")
}

fn render_service_unit(relay_bin: &Path, cred_mode: &CredentialMode) -> String {
    let cred_line = match cred_mode {
        CredentialMode::Encrypted(path) => {
            format!("LoadCredentialEncrypted=beszel-password:{}", path.display())
        }
        CredentialMode::PlainFile(path) => {
            format!("LoadCredential=beszel-password:{}", path.display())
        }
    };

    format!(
        "[Unit]\n\
Description=Beszel Relay per-user realtime metrics daemon\n\
After=network-online.target\n\
Wants=network-online.target\n\
\n\
[Service]\n\
Type=simple\n\
ExecStart={}\n\
{}\n\
Restart=on-failure\n\
RestartSec=3\n\
UMask=0077\n\
Environment=RUST_LOG=info\n\
\n\
[Install]\n\
WantedBy=default.target\n",
        relay_bin.display(),
        cred_line
    )
}

fn install_and_start_service(home: &Path, cred_mode: &CredentialMode) -> Result<PathBuf> {
    let unit_dir = home.join(".config").join("systemd").join("user");
    fs::create_dir_all(&unit_dir)
        .with_context(|| format!("failed to create {}", unit_dir.display()))?;

    let relay_bin = resolve_relay_binary(home);
    let unit_path = unit_dir.join("beszel-relay.service");
    let content = render_service_unit(&relay_bin, cred_mode);
    fs::write(&unit_path, content)
        .with_context(|| format!("failed to write {}", unit_path.display()))?;

    run_systemctl(&["--user", "daemon-reload"])?;
    run_systemctl(&["--user", "enable", "beszel-relay.service"])?;
    run_systemctl(&["--user", "restart", "beszel-relay.service"])?;
    Ok(unit_path)
}

fn run_systemctl(args: &[&str]) -> Result<()> {
    let status = Command::new("systemctl")
        .args(args)
        .status()
        .with_context(|| format!("failed to execute systemctl {}", args.join(" ")))?;
    if !status.success() {
        bail!("systemctl {} exited with status {}", args.join(" "), status);
    }
    Ok(())
}

async fn wait_for_socket(socket_path: &Path) {
    for _ in 0..15 {
        if UnixStream::connect(socket_path).await.is_ok() {
            println!("Relay socket active at {}", socket_path.display());
            return;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    println!(
        "Note: beszel-relay is still starting; check `systemctl --user status beszel-relay` if needed."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_valid_hub_urls_and_rejects_invalid_ones() {
        assert_eq!(
            normalize_hub_url("  https://beszel.example.com/// ").unwrap(),
            "https://beszel.example.com"
        );
        assert_eq!(
            normalize_hub_url("http://192.168.1.5:8090/").unwrap(),
            "http://192.168.1.5:8090"
        );
        assert!(normalize_hub_url("beszel.example.com").is_err());
        assert!(normalize_hub_url("https://").is_err());
    }

    #[test]
    fn renders_config_toml_without_password() {
        let creds = HubCredentials {
            hub_url: "https://beszel.example.com".into(),
            email: "user@example.com".into(),
            password: "super-secret-password".into(),
            system: Some("workstation,server".into()),
        };
        let toml_text = render_config_toml(&creds).expect("config renders");
        assert!(toml_text.contains("hub_url = \"https://beszel.example.com\""));
        assert!(toml_text.contains("email = \"user@example.com\""));
        assert!(toml_text.contains("system = \"workstation,server\""));
        assert!(!toml_text.contains("super-secret-password"));
    }

    #[test]
    fn renders_encrypted_and_plain_service_units() {
        let bin = Path::new("/home/user/.local/bin/beszel-relay");

        let encrypted = render_service_unit(
            bin,
            &CredentialMode::Encrypted(PathBuf::from(
                "/home/user/.config/beszel-desktop/beszel-password.cred",
            )),
        );
        assert!(encrypted.contains("ExecStart=/home/user/.local/bin/beszel-relay"));
        assert!(encrypted.contains(
            "LoadCredentialEncrypted=beszel-password:/home/user/.config/beszel-desktop/beszel-password.cred"
        ));

        let plain = render_service_unit(
            bin,
            &CredentialMode::PlainFile(PathBuf::from(
                "/home/user/.config/beszel-desktop/beszel-password",
            )),
        );
        assert!(plain.contains(
            "LoadCredential=beszel-password:/home/user/.config/beszel-desktop/beszel-password"
        ));
    }
}
