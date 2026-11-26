# Waybar Adapter

Tested on Wayland (niri) with Waybar 0.15.0.

`beszel-ctl waybar` connects to the local `beszel-relay` Unix socket (`$XDG_RUNTIME_DIR/beszel-relay.sock`) and streams continuous Waybar JSON lines on stdout without needing hub credentials.

## Setup

1. Install and start `beszel-relay` + `beszel-ctl` (see [packaging/README.md](../../packaging/README.md)), ensuring `~/.local/bin` is on Waybar's `PATH` (or use the full path `~/.local/bin/beszel-ctl`).
2. Add `"custom/beszel"` to your bar layout and merge [config.jsonc](config.jsonc) into `~/.config/waybar/config.jsonc`.
3. Merge [style.css](style.css) into `~/.config/waybar/style.css`.
4. Reload Waybar (`pkill -SIGUSR2 waybar` or restart your Waybar service).

## CSS Classes

`beszel-ctl waybar` sets the JSON `class` field to reflect live health:
- `connected`: live samples arriving within `--stale-secs` (default 5s)
- `stale`: connected, but no sample received within `--stale-secs`
- `connecting` / `reconnecting`: `beszel-relay` is connecting or re-registering `rt_metrics` after a hub outage
- `disconnected`: `beszel-relay` socket unavailable (automatically retries every 2s)
- `error`: hub authentication error reported by `beszel-relay`
