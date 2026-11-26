# Packaging & User Service Setup

## Quick Install & Setup

Download SHA-256-verified release binaries into `~/.local/bin` and launch the interactive `beszel-ctl init` wizard:

```sh
curl -fsSL https://raw.githubusercontent.com/henriksvensson/beszel-desktop/main/packaging/install.sh | sh
```

Pass `--no-init` to install binaries without running `beszel-ctl init`, or `--version v0.3.0` to pin a release tag.

## Updating

For `~/.local/bin` installs, run:

```sh
beszel-ctl update        # downloads & verifies latest release, swaps binaries, restarts service
beszel-ctl update --check
```

Distro-packaged installs (`/usr/bin`, `/nix/store`) are updated through your package manager (`dnf upgrade`, `yay -Syu`, `nix profile upgrade`).

## Manual Build & Setup

Install `beszel-relay` and `beszel-ctl` from source:

```sh
cargo build --release --workspace
install -Dm755 target/release/beszel-relay ~/.local/bin/beszel-relay
install -Dm755 target/release/beszel-ctl ~/.local/bin/beszel-ctl
```

Run the interactive wizard (`~/.local/bin/beszel-ctl init`), or configure manually:

Create a user-only config file (no plaintext password needed):

```sh
install -dm700 ~/.config/beszel-desktop
install -Dm600 packaging/config.example.toml ~/.config/beszel-desktop/config.toml
$EDITOR ~/.config/beszel-desktop/config.toml
```

Encrypt the Beszel account password with `systemd-creds` (requires `systemd >= 256` for `--user`; uses TPM2 + host key when available):

```sh
systemd-creds encrypt --user --name=beszel-password - ~/.config/beszel-desktop/beszel-password.cred
```

> **Note for `systemd < 256` (e.g., Ubuntu 24.04 / `systemd 255`):** `systemd-creds encrypt --user` is not available yet. Store the password in a user-only file (`chmod 600 ~/.config/beszel-desktop/beszel-password`) and change `LoadCredentialEncrypted=beszel-password:%h/.config/beszel-desktop/beszel-password.cred` in `~/.config/systemd/user/beszel-relay.service` to `LoadCredential=beszel-password:%h/.config/beszel-desktop/beszel-password` (or set `password_file` in `config.toml`).

Install and start the systemd user service (which mounts the credential in memory at `$CREDENTIALS_DIRECTORY/beszel-password`):

```sh
install -Dm644 packaging/systemd/beszel-relay.service ~/.config/systemd/user/beszel-relay.service
systemctl --user daemon-reload
systemctl --user enable --now beszel-relay.service
```

Verify the local Unix socket stream:

```sh
~/.local/bin/beszel-ctl status
~/.local/bin/beszel-ctl watch
```
