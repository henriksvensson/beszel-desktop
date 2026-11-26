# Packaging & User Service Setup

Install `beszel-relay` and `beszel-ctl` for your user:

```sh
cargo build --release --workspace
install -Dm755 target/release/beszel-relay ~/.local/bin/beszel-relay
install -Dm755 target/release/beszel-ctl ~/.local/bin/beszel-ctl
```

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
