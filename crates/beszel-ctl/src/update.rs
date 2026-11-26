use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use clap::Args;
use serde::Deserialize;

const REPO: &str = "henriksvensson/beszel-desktop";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Args)]
pub struct UpdateArgs {
    /// Check whether a newer release is available without installing it.
    #[arg(long)]
    pub check: bool,
    /// Install a specific release tag (e.g. v0.3.0) instead of the latest release.
    #[arg(long)]
    pub version: Option<String>,
    /// Reinstall even when already running the target version.
    #[arg(long)]
    pub force: bool,
    /// Do not restart beszel-relay.service after replacing binaries.
    #[arg(long)]
    pub no_restart: bool,
}

#[derive(Debug, Deserialize)]
struct ReleaseInfo {
    tag_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct SemVer(u64, u64, u64);

struct ReleaseSpec {
    tag: String,
    arch_triple: &'static str,
}

struct TempDirGuard(PathBuf);

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub async fn run_update(args: UpdateArgs) -> Result<()> {
    let client = build_http_client()?;
    let target_tag = resolve_target_tag(&client, args.version.as_deref()).await?;

    let current = parse_semver(CURRENT_VERSION)?;
    let target = parse_semver(&target_tag)?;
    let has_update = target > current;

    if args.check {
        print_check_result(&target_tag, has_update);
        return Ok(());
    }

    if should_skip_update(has_update, &args) {
        println!("beszel-desktop is already up to date (v{CURRENT_VERSION}).");
        return Ok(());
    }

    let install_dir = resolve_writable_install_dir()?;
    let spec = ReleaseSpec {
        tag: target_tag,
        arch_triple: linux_target_triple(std::env::consts::ARCH)?,
    };
    install_release(&client, &spec, &install_dir).await?;

    println!(
        "Updated beszel-relay and beszel-ctl to {} in {}",
        spec.tag,
        install_dir.display()
    );

    if !args.no_restart {
        try_restart_relay_service();
    }
    Ok(())
}

fn should_skip_update(has_update: bool, args: &UpdateArgs) -> bool {
    if has_update || args.force {
        return false;
    }
    args.version.is_none()
}

fn build_http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .user_agent(format!("beszel-ctl/{CURRENT_VERSION}"))
        .build()
        .context("failed to build HTTP client")
}

async fn resolve_target_tag(client: &reqwest::Client, requested: Option<&str>) -> Result<String> {
    if let Some(tag) = requested {
        return Ok(normalize_tag(tag));
    }
    fetch_latest_tag(client).await
}

async fn fetch_latest_tag(client: &reqwest::Client) -> Result<String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let resp = client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .context("failed to query GitHub Releases")?;

    if !resp.status().is_success() {
        bail!("GitHub Releases API returned HTTP {}", resp.status());
    }

    let info: ReleaseInfo = resp
        .json()
        .await
        .context("unexpected GitHub Releases response")?;
    Ok(normalize_tag(&info.tag_name))
}

fn print_check_result(target_tag: &str, has_update: bool) {
    if has_update {
        println!(
            "Update available: v{CURRENT_VERSION} -> {target_tag}. Run `beszel-ctl update` to install."
        );
    } else {
        println!("Up to date: v{CURRENT_VERSION} (latest release: {target_tag}).");
    }
}

fn normalize_tag(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.starts_with('v') {
        trimmed.to_string()
    } else {
        format!("v{trimmed}")
    }
}

fn parse_semver(tag: &str) -> Result<SemVer> {
    let stripped = tag.trim().trim_start_matches('v');
    let nums: Vec<u64> = stripped
        .split('.')
        .map(|part| {
            part.parse::<u64>()
                .with_context(|| format!("invalid version component in {tag}"))
        })
        .collect::<Result<_>>()?;

    let [major, minor, patch] = nums.as_slice() else {
        bail!("unsupported version format: {tag}");
    };
    Ok(SemVer(*major, *minor, *patch))
}

fn linux_target_triple(arch: &str) -> Result<&'static str> {
    match arch {
        "x86_64" => Ok("x86_64-unknown-linux-gnu"),
        "aarch64" => Ok("aarch64-unknown-linux-gnu"),
        other => bail!("prebuilt releases are only available for x86_64 and aarch64 (got {other})"),
    }
}

fn detect_package_manager_hint(dir: &Path) -> Option<&'static str> {
    if dir.starts_with("/nix/store") {
        return Some(
            "Installed via Nix — update with `nix profile upgrade` or `nix flake update`.",
        );
    }
    const SYSTEM_ROOTS: [&str; 4] = ["/usr", "/bin", "/sbin", "/opt"];
    if SYSTEM_ROOTS.iter().any(|root| dir.starts_with(root)) {
        return Some(
            "Installed via a system package manager — update with `dnf upgrade`, `yay -Syu`, or your distro's updater.",
        );
    }
    None
}

fn resolve_writable_install_dir() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("failed to determine current executable path")?;
    let canonical = fs::canonicalize(&exe).unwrap_or(exe);
    let dir = canonical
        .parent()
        .context("executable has no parent directory")?
        .to_path_buf();

    if let Some(hint) = detect_package_manager_hint(&dir) {
        bail!("{hint}");
    }
    Ok(dir)
}

impl ReleaseSpec {
    fn package_dir_name(&self) -> String {
        format!("beszel-desktop-{}-{}", self.tag, self.arch_triple)
    }

    fn tarball_name(&self) -> String {
        format!("{}.tar.gz", self.package_dir_name())
    }

    fn asset_url(&self, asset_name: &str) -> String {
        format!(
            "https://github.com/{REPO}/releases/download/{}/{asset_name}",
            self.tag
        )
    }
}

async fn install_release(
    client: &reqwest::Client,
    spec: &ReleaseSpec,
    install_dir: &Path,
) -> Result<()> {
    let tarball = spec.tarball_name();
    let tmp = create_temp_dir()?;
    let tarball_path = tmp.0.join(&tarball);
    let sums_path = tmp.0.join("SHA256SUMS.target");

    println!("Downloading {tarball}...");
    download_asset(client, &spec.asset_url(&tarball), &tarball_path).await?;
    let sums_text = download_checksums(client, spec).await?;

    let checksum_line = extract_checksum_line(&sums_text, &tarball)?;
    fs::write(&sums_path, format!("{checksum_line}\n"))
        .context("failed to write temporary checksum file")?;

    println!("Verifying SHA-256 checksum...");
    verify_checksum(&tmp.0, &sums_path)?;
    extract_tarball(&tmp.0, &tarball_path)?;

    let extracted = tmp.0.join(spec.package_dir_name());
    atomic_install_binary(
        &extracted.join("beszel-relay"),
        &install_dir.join("beszel-relay"),
    )?;
    atomic_install_binary(
        &extracted.join("beszel-ctl"),
        &install_dir.join("beszel-ctl"),
    )?;
    Ok(())
}

fn create_temp_dir() -> Result<TempDirGuard> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("beszel-update-{}-{stamp}", std::process::id()));
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    Ok(TempDirGuard(dir))
}

async fn download_asset(client: &reqwest::Client, url: &str, dest: &Path) -> Result<()> {
    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("failed to download {url}"))?;
    if !resp.status().is_success() {
        bail!("failed to download {url} (HTTP {})", resp.status());
    }
    let bytes = resp.bytes().await.context("failed reading download body")?;
    fs::write(dest, &bytes).with_context(|| format!("failed writing {}", dest.display()))?;
    Ok(())
}

async fn download_checksums(client: &reqwest::Client, spec: &ReleaseSpec) -> Result<String> {
    let url = spec.asset_url("SHA256SUMS");
    let resp = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("failed to download {url}"))?;
    if !resp.status().is_success() {
        bail!("failed to download {url} (HTTP {})", resp.status());
    }
    resp.text().await.context("failed reading SHA256SUMS")
}

fn extract_checksum_line(sums: &str, filename: &str) -> Result<String> {
    let suffix = format!("  {filename}");
    sums.lines()
        .find(|line| line.ends_with(&suffix))
        .map(str::to_string)
        .with_context(|| format!("checksum for {filename} not found in SHA256SUMS"))
}

fn verify_checksum(work_dir: &Path, sums_file: &Path) -> Result<()> {
    let status = Command::new("sha256sum")
        .arg("-c")
        .arg(sums_file)
        .current_dir(work_dir)
        .status()
        .context("failed to run sha256sum")?;
    if !status.success() {
        bail!("SHA-256 verification failed");
    }
    Ok(())
}

fn extract_tarball(work_dir: &Path, tarball: &Path) -> Result<()> {
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(tarball)
        .arg("-C")
        .arg(work_dir)
        .status()
        .context("failed to run tar")?;
    if !status.success() {
        bail!("failed to extract release archive");
    }
    Ok(())
}

fn atomic_install_binary(src: &Path, dest: &Path) -> Result<()> {
    let file_name = dest
        .file_name()
        .and_then(|n| n.to_str())
        .context("invalid target binary path")?;
    let staged = dest.with_file_name(format!(".{file_name}.new.{}", std::process::id()));

    fs::copy(src, &staged)
        .with_context(|| format!("failed staging {} into {}", src.display(), staged.display()))?;
    fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))
        .with_context(|| format!("failed chmod 755 {}", staged.display()))?;
    fs::rename(&staged, dest).with_context(|| format!("failed replacing {}", dest.display()))?;
    Ok(())
}

fn try_restart_relay_service() {
    let status = Command::new("systemctl")
        .args(["--user", "try-restart", "beszel-relay.service"])
        .status();
    if let Ok(st) = status
        && st.success()
    {
        println!("Restarted beszel-relay.service.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_semver_tags_correctly() {
        assert_eq!(parse_semver("v0.2.0").unwrap(), SemVer(0, 2, 0));
        assert_eq!(parse_semver("0.10.1").unwrap(), SemVer(0, 10, 1));
        assert!(parse_semver("v0.10.0").unwrap() > parse_semver("v0.2.9").unwrap());
        assert!(parse_semver("v1.0.0").unwrap() > parse_semver("v0.99.99").unwrap());
        assert!(parse_semver("v0.2").is_err());
    }

    #[test]
    fn extracts_matching_sha256_line() {
        let sums = "\
aaaaaaaaaaaaaaaa  beszel-desktop-v0.3.0-aarch64-unknown-linux-gnu.tar.gz\n\
bbbbbbbbbbbbbbbb  beszel-desktop-v0.3.0-x86_64-unknown-linux-gnu.tar.gz\n";
        let line = extract_checksum_line(
            sums,
            "beszel-desktop-v0.3.0-x86_64-unknown-linux-gnu.tar.gz",
        )
        .expect("matching line");
        assert!(line.starts_with("bbbbbbbbbbbbbbbb"));
        assert!(extract_checksum_line(sums, "missing.tar.gz").is_err());
    }

    #[test]
    fn detects_package_manager_directories() {
        assert!(detect_package_manager_hint(Path::new("/usr/bin")).is_some());
        assert!(detect_package_manager_hint(Path::new("/nix/store/abc-beszel/bin")).is_some());
        assert!(detect_package_manager_hint(Path::new("/home/user/.local/bin")).is_none());
    }
}
