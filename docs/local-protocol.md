# Local Unix Socket Protocol (NDJSON)

`beszel-relay` exposes a user-only (`0600`) Unix domain socket at `$XDG_RUNTIME_DIR/beszel-relay.sock` (overridable via `--socket` or `BESZEL_SOCKET_PATH`).

Each message is a single compact JSON object terminated by `\n` (newline-delimited JSON). Hub credentials never cross this socket.

## Message Flow

1. Immediately on connect, `beszel-relay` sends a `snapshot` message containing the current hub connection status and each subscribed system's bounded live rolling history (ordered oldest to newest).
2. Afterwards, `beszel-relay` streams incremental `sample`, `connection`, and `systems_changed` events as they occur.
3. If a slow client falls behind the internal broadcast buffer, `beszel-relay` sends a fresh `snapshot` so the client recovers without gaps or stale state.

## Event Types (`"type"`)

### `snapshot`
```json
{
  "type": "snapshot",
  "protocol_version": 1,
  "generated_at_ms": 1700000005000,
  "connection": {
    "state": "connected",
    "since_ms": 1700000000000,
    "last_sample_ms": 1700000004900,
    "reconnect_attempts": 0
  },
  "systems": [
    {
      "system": {
        "id": "sys_example_01",
        "name": "workstation",
        "status": "up",
        "agent_version": "0.21.2"
      },
      "history_capacity": 60,
      "samples": [
        {
          "timestamp_ms": 1700000004900,
          "cpu_pct": 4.95,
          "cpu_cores": [0, 0, 0, 0, 8, 8, 9, 8],
          "mem_pct": 13.45,
          "mem_used_bytes": 8998420480,
          "mem_total_bytes": 66916192256,
          "mem_buff_cache_bytes": 14000463872,
          "swap_used_bytes": 0,
          "swap_total_bytes": 8589930496
        }
      ]
    }
  ]
}
```

### `sample`
```json
{
  "type": "sample",
  "system_id": "sys_example_01",
  "sample": {
    "timestamp_ms": 1700000005900,
    "cpu_pct": 3.67,
    "mem_pct": 13.44,
    "mem_used_bytes": 8995975168,
    "mem_total_bytes": 66916192256,
    "mem_buff_cache_bytes": 14000463872,
    "swap_used_bytes": 0,
    "swap_total_bytes": 8589930496
  }
}
```

### `connection`
```json
{
  "type": "connection",
  "connection": {
    "state": "reconnecting",
    "since_ms": 1700000010000,
    "last_sample_ms": 1700000005900,
    "last_error": "SSE stream closed by hub",
    "reconnect_attempts": 1
  }
}
```

Valid `connection.state` values: `connecting`, `connected`, `paused`, `reconnecting`, `disconnected`, `auth_error`.

`paused` indicates the hub SSE connection is listening for `systems/*` changes while `rt_metrics` polling is paused because zero local clients are connected.

### `systems_changed`
```json
{
  "type": "systems_changed",
  "systems": [
    {
      "id": "sys_example_01",
      "name": "workstation",
      "status": "up",
      "agent_version": "0.21.2"
    }
  ]
}
```

### `alerts_changed`
```json
{
  "type": "alerts_changed",
  "alerts": [
    {
      "id": "alert_01",
      "system_id": "sys_example_01",
      "name": "Disk",
      "value": 85.0,
      "min": 1
    }
  ],
  "alert_counts": {
    "sys_example_01": 6
  }
}
```

## Live vs. Historical Model

This schema intentionally represents only the ~1 Hz live stream (`LiveSample` / `SystemLiveState`). Any future historical REST backfill from Beszel's stored stats collections will be modeled separately so different sampling cadences do not collide.
