# DankMaterialShell Plugin

Tested on Wayland (niri) with DankMaterialShell 2.1.2 and Quickshell 0.3.2.

This widget connects directly to the local `beszel-relay` Unix domain socket (`$XDG_RUNTIME_DIR/beszel-relay.sock`) via `Quickshell.Io` `Socket` + `SplitParser`. Hub credentials stay inside `beszel-relay`.

## Setup

1. Install and start `beszel-relay` (see [packaging/README.md](../../packaging/README.md)).
2. Symlink this plugin directory into DMS's user plugins directory:
   ```sh
   mkdir -p ~/.config/dms/plugins
   ln -sfn /path/to/beszel-desktop/shells/dms ~/.config/dms/plugins/beszelDesktop
   ```
3. Enable **Beszel Desktop** in DMS Settings → Plugins and add it to your DankBar.
4. During development, reload the plugin without restarting DMS:
   ```sh
   dms ipc call plugins reload beszelDesktop
   ```

## Behavior

- **DankBar pill:** displays `<host>` with compact CPU (`memory`) and RAM (`developer_board`) icons and fixed-width zero-padded (`00%`–`99%`) percentages with smooth severity color transitions (amber at warn threshold, red at danger threshold).
- **One-click host discovery & variants:** DMS Settings → Plugins → Beszel Desktop connects to `beszel-relay.sock` to list discovered hosts and add independent `Beszel Desktop - <host>` DankBar variants with one click.
- **Slim real-time popout:** shows host switcher chips with live severity dots, smooth hover-scrubbable CPU & memory area sparklines with gap breaks (`> 3.5s`), and an animated per-core CPU heatmap with per-core hover inspection.
- **Resilience:** automatically reconnects every 2s if `beszel-relay` restarts and re-hydrates rolling history from the initial `snapshot` frame.
