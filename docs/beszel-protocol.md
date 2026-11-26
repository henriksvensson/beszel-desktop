# Beszel Realtime Protocol Notes

Verified against a deployed Beszel hub on 2026-11-25.

## Verified Versions

- **Hub:** `0.21.2` (confirmed via authenticated `GET /api/beszel/info` -> `{"v": "0.21.2", ...}` and root HTML `HUB_VERSION`)
- **Agents:** `0.21.2` across tested `up` systems (Beszel requires agent `>= 0.13.0` for 1-minute live `rt_metrics`)

## Authentication & Discovery

1. **Authenticate user:**
   - `POST /api/collections/users/auth-with-password`
   - Body: `{"identity": "<email>", "password": "<password>"}`
   - Returns `200 OK` with `token` and `record`.
   - Pass `Authorization: <token>` on authenticated API requests.
2. **List accessible systems:**
   - `GET /api/collections/systems/records?perPage=50`
   - Returns `items[]` with `id` (15-char PocketBase record ID), `name`, `host`, `port`, `status` (`"up"` / `"down"` / ...), and summary `info` (including agent version `info.v`).

## Live SSE Subscription (`rt_metrics`)

1. **Open SSE stream:**
   - `GET /api/realtime` with `Accept: text/event-stream`.
   - Returns `200 OK` (`Content-Type: text/event-stream`).
   - First event is `event: PB_CONNECT` with `data: {"clientId":"<40-char id>"}`.
2. **Register subscription:**
   - `POST /api/realtime` with `Authorization: <token>` and JSON body:
     ```json
     {
       "clientId": "<clientId>",
       "subscriptions": [
         "rt_metrics?options=%7B%22query%22%3A%7B%22system%22%3A%22<SYSTEM_ID>%22%7D%7D"
       ]
     }
     ```
   - `options` is URL-encoded compact JSON: `{"query":{"system":"<SYSTEM_ID>"}}`.
   - Returns `204 No Content`.
3. **Receive live samples:**
   - The hub polls the agent about once per second (`0.93s`–`1.05s` observed intervals).
   - Each SSE message uses `event:` equal to the exact subscribed topic string (`rt_metrics?options=%7B%22query%22%3A%7B%22system%22%3A%22<SYSTEM_ID>%22%7D%7D`).
   - Messages have no server-side sample timestamp; `beszel-relay` timestamps each received sample locally.

## Reconnect & Resubscription

Verified by closing the active `GET /api/realtime` connection, opening a new stream, and resubscribing:
- Reconnecting produces a new, distinct `clientId` in `PB_CONNECT`.
- Sending `POST /api/realtime` with the new `clientId` and the same `subscriptions` list returns `204 No Content`.
- Live ~1 Hz `rt_metrics` events resume immediately (`0.95s` observed interval).

## Live Payload Shape

See [fixtures/rt_metrics_sample.json](fixtures/rt_metrics_sample.json) for two consecutive sanitized samples.

Top-level JSON fields observed on `0.21.2`:
- `stats` (`object`): detailed live system metrics
- `info` (`object`): summary fields (`v` agent version, `u` uptime seconds, `cpu`, `mp`, `dp`, `la`, `bb`, `dt`, `bat`, `wf`, etc.)
- `container` (`array`): live container metrics (`[]` when none are running)
- `Details` (`null | object`): optional system details
- `Monitors` (`object`): optional monitor stats map (`{}` when empty)

### Key `stats` Fields for the First Working Slice

- **CPU:**
  - `cpu` (`f64`): total CPU utilization percent
  - `cpus` (`Vec<u8>`, optional): per-core utilization percent
  - `cpub` (`Vec<f64>`, optional): `[user, system, iowait, steal, idle, irq, softirq, nice]`
- **Memory:**
  - `mp` (`f64`): memory utilization percent
  - `mem` (`[u64; 4]`, optional on older agents): `[total_bytes, used_bytes, buff_cache_bytes, zfs_arc_bytes]` — preferred byte-accurate representation
  - `swap` (`[u64; 2]`, optional): `[total_bytes, used_bytes]`
  - `m`, `mu`, `mb`, `s`, `su` (`f64`, deprecated): memory/swap in GB kept for older agents; use as fallback only when `mem` is absent

### Defensive Decoding Notes

- Upstream Go structs use `omitzero` / `omitempty` extensively; fields such as `dio`, `dios`, `su`, `g`, `bat`, `wf`, `f`, and `z` may appear on one tick and be omitted on the next when zero or unavailable.
- Always deserialize upstream fields permissively (optional fields, ignore unknown keys) so minor Beszel releases do not break decoding.
