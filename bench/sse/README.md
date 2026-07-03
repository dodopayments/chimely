# SSE concurrency load harness

Measures how many concurrent `GET /v1/inbox/stream` connections a Chimely
server holds, and the hint fan-in latency under that load: the time from a
`POST /v1/notifications` being accepted to the `hint` SSE event arriving on
the target subscriber's open streams.

Plain Node (no npm deps, raw `node:http`, one dedicated socket per stream).

## Run

Against any Chimely instance:

```bash
ulimit -n 16384   # see "File descriptors" below

SERVER_PID=$(pgrep -f 'chimely serve' | head -1) \
CHIMELY_URL=http://127.0.0.1:8299 \
CHIMELY_ENV=demo \
CHIMELY_API_KEY=dev-secret-key \
N=2000 M=250 T=90 \
node bench/sse/sse-load.mjs
```

| Var | Default | Meaning |
| --- | --- | --- |
| `CHIMELY_URL` | `http://127.0.0.1:8299` | Base URL of the server under test |
| `CHIMELY_ENV` | `demo` | Environment slug (subscriber auth via `X-Chimely-Environment` header) |
| `CHIMELY_API_KEY` | `dev-secret-key` | Management bearer key for the probe POSTs |
| `N` | `500` | Total concurrent SSE connections |
| `M` | `ceil(N/CAP)` | Distinct subscribers; connections round-robin across them |
| `CAP` | `8` | Per-subscriber connection cap; must match the server's `CHIMELY_SSE_MAX_CONNS_PER_SUBSCRIBER`. The harness refuses to start if `N/M > CAP` |
| `T` | `90` | Seconds to hold all connections after the ramp completes |
| `PROBE_INTERVAL_MS` | `2000` | Probe cadence. Keep it above the server's `CHIMELY_HINT_DEBOUNCE_MS` (default 1000) or the debounce coalesces probes and the latency numbers lie |
| `RAMP_CONCURRENCY` | `100` | Parallel connection-open attempts during the ramp |
| `SERVER_PID` | unset | Local server pid; RSS is sampled via `ps` every 5 s. Unset skips RSS (e.g. remote target) |
| `SUB_PREFIX` | `bench-sub` | Subscriber id prefix (`bench-sub-0` … `bench-sub-(M-1)`) |
| `OUT_JSON` | unset | Also write the JSON summary to this path |

Progress goes to stderr; the final JSON summary goes to stdout.

The probe subscriber is always `<SUB_PREFIX>-0`, which holds `ceil(N/M)`
connections, so each accepted probe should yield that many latency samples.
Latency is measured from probe-POST response received to `hint` event
observed, clamped at 0 (the outbox worker can publish between the server's
transaction commit and the POST response flushing back).

If the target environment requires subscriber hashes
(`require_subscriber_hash = true`), this harness cannot authenticate: it
sends no `X-Chimely-Subscriber-Hash`. Use a dev-bootstrapped environment
(`CHIMELY_DEV_ENVIRONMENT`) or one with hashes disabled.

Server-side prerequisites for a clean measurement:

- `CHIMELY_SUBSCRIBER_RATE_PER_SEC=0` on the server, or the subscriber-plane
  token bucket (default 10/s, burst 50) rejects the ramp itself.
- The probe rate (default 0.5/s) sits far below the management-plane key
  limit (default 50/s), so no server tuning is needed there.

## What the numbers mean

- `established` vs `N`: connections that got a 200 and stayed up. Failures
  are broken down by cause (`http_429` = per-subscriber cap or rate limit,
  `connect_*` = socket-level).
- `hint_latency_ms`: end-to-end hint plumbing under load: management POST ->
  transactional outbox -> worker poll (`CHIMELY_WORKER_POLL_MS`, default
  250 ms) -> pub/sub (Redis, or Postgres LISTEN/NOTIFY in Redis-less mode)
  -> broadcast fan-in -> SSE write. The worker poll interval is an expected
  floor component: ~half the poll interval on average before load is even a
  factor.
- `server_rss_mb`: resident set of the server process, sampled every 5 s.
- A run only counts as "held" if `open_at_end` equals `established` and
  `dropped_mid_stream` is 0 for the whole window.

## The laptop ceiling

Numbers from a laptop bound what THIS laptop can demonstrate, not what the
server can do. The honest ceilings you hit locally, roughly in the order
they appear:

1. **File descriptors.** Every SSE connection is one fd on the client AND
   one on the server. macOS defaults are low (`ulimit -n` is often 256 or
   10240); the hard per-process cap is `sysctl kern.maxfilesperproc`
   (commonly 61440) and system-wide `kern.maxfiles`. Raise the soft limit in
   BOTH shells (server and harness) before running:
   `ulimit -n 16384` (or higher). Linux: also check
   `/proc/sys/fs/file-max` and `nofile` in limits.conf/systemd.
2. **Ephemeral ports.** Client and server share one loopback; each
   connection consumes a local port from `net.inet.ip.portrange`
   (~16k usable on macOS by default). Above ~15k connections from one
   client IP to one server port, you exhaust source ports.
3. **The harness itself.** One Node process parsing N streams competes with
   the server for the same cores. Keep-alive pings (`CHIMELY_SSE_PING_SECS`,
   default 30) mean N/30 frames/s of background parse work.
4. **Memory.** Per-connection cost on the server is a broadcast receiver +
   stream state. Watch `server_rss_mb` growth across ramp steps; it should
   be roughly linear in open connections.

Client and server co-located also means latency numbers exclude real network
RTT, and a single loopback interface serializes what a real NIC + kernel
would spread out.

## What to expect in a real environment

- A production deployment terminates TLS in front of the binary; TLS adds
  per-connection memory (~tens of KB in the terminator) and handshake cost
  on ramp, none of which this harness exercises.
- Hint latency in production includes real network RTT and, with Redis
  configured, Redis pub/sub instead of the LISTEN/NOTIFY fallback this
  harness may have measured (see the run report). The dominant term stays
  the worker poll interval unless the outbox backs up.
- Multiple replicas divide connections; the per-subscriber cap is per
  replica, so a load balancer changes effective per-subscriber limits.
- Real clients are EventSource: they reconnect with `Last-Event-ID`,
  triggering the resume-hint query per reconnect. This harness holds
  steady-state connections and does not model reconnect storms (the server
  jitters `retry:` on deploys precisely to spread those).
- Fd limits on a server distro are a systemd `LimitNOFILE` setting, not a
  shell ulimit.
