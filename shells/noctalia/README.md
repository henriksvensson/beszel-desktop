# Beszel Desktop

Monitor remote servers, homelab nodes, and other machines live (~1 Hz) from a [Beszel](https://beszel.dev) hub — rendered in compact Noctalia bar pills and an attached real-time telemetry panel with CPU/memory sparklines and a per-core heatmap.

<p align="center">
  <img src="../../docs/assets/noctalia-panel.gif" alt="Beszel Desktop live telemetry panel in Noctalia Shell" width="374">
</p>

## Plugin

| Field | Value |
| --- | --- |
| ID | `henriksvensson/beszel-desktop` |
| Entries | Bar widget: `bar`; panel: `panel`; service: `service` |

## Requirements

- Install [`beszel-relay`](https://github.com/henriksvensson/beszel-desktop) and `beszel-ctl` on `PATH` (or in `~/.local/bin/`).
- Start the `beszel-relay` user service configured against a Beszel hub (`>= 0.13.0`). See [packaging/README.md](../../packaging/README.md).
- Noctalia Shell `v5.5+` (`plugin_api = 48`).

## Usage

1. Install and enable `henriksvensson/beszel-desktop` in **Noctalia Settings → Plugins** (or install manually by linking `shells/noctalia` to `~/.local/share/noctalia/plugins/beszel-desktop`).
2. Add one or more independent host widgets to your bar in `~/.config/noctalia/config.toml`:
   ```toml
   [widget.beszel-workstation]
   type = "henriksvensson/beszel-desktop:bar"
   system = "workstation"

   [widget.beszel-server]
   type = "henriksvensson/beszel-desktop:bar"
   system = "server"
   ```
   Then add `"beszel-workstation"` / `"beszel-server"` to your bar's widget list.
3. Click any host pill to open the attached telemetry panel focused on that host (with host switcher chips, hover-scrubbable CPU & memory graphs, and a frame-interpolated per-core CPU heatmap), or toggle the panel via IPC:

```sh
noctalia msg panel-toggle henriksvensson/beszel-desktop:panel
```

## Settings

### Plugin Settings

| Setting | Type | Default | Description |
| --- | --- | --- | --- |
| `ctl_path` | `string` | `"beszel-ctl"` | Path or binary name for `beszel-ctl` (also checks `~/.local/bin/beszel-ctl`). |
| `socket_path` | `string` | `""` | Optional override for the `beszel-relay` Unix socket path. |
| `stale_seconds` | `int` | `5` | Seconds without a new sample before metrics are marked stale (`2`–`60`). |
| `warn_threshold` | `int` | `70` | Percentage threshold (`10`–`99`) for warning severity coloring. |
| `danger_threshold` | `int` | `90` | Percentage threshold (`11`–`100`) for danger severity coloring. |

### Bar Widget Settings (`henriksvensson/beszel-desktop:bar`)

| Setting | Type | Default | Description |
| --- | --- | --- | --- |
| `system` | `string` | `""` | Beszel system name or ID to display (leave empty for the first available host). |
| `show_host_name` | `bool` | `true` | Show the host name prefix before the CPU and memory readouts. |

## Notes

- **Spawned process:** The singleton `service` entry runs one long-lived `beszel-ctl watch --raw --reconnect` process (reading from the local user-only Unix socket `$XDG_RUNTIME_DIR/beszel-relay.sock`) and shares parsed snapshots/samples through `noctalia.state`.
- **Network & credentials:** This plugin makes no network requests, writes no files, and never accesses Beszel hub credentials (credentials stay inside `beszel-relay`).
- **Auto-pause:** Disabling the plugin stops the `beszel-ctl` stream, allowing `beszel-relay` to pause upstream `rt_metrics` polling automatically.
