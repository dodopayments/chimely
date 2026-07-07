#!/usr/bin/env node

// SSE concurrency load harness for Chimely.
//
// Opens N concurrent SSE connections to GET /v1/inbox/stream spread across M
// distinct subscribers (N/M must stay at or below the per-subscriber cap,
// CHIMELY_SSE_MAX_CONNS_PER_SUBSCRIBER, default 8). Holds them for T seconds.
// While holding, it periodically POSTs a notification to one probe subscriber
// via the management API and measures hint fan-in latency: the time from the
// POST response ("accepted") to the `hint` SSE event observed on each of that
// subscriber's open connections.
//
// Plain Node, no dependencies. Raw `node:http` with `agent: false` so every
// SSE stream owns a dedicated socket (one fd per connection, no pooling).
//
// Usage:
//   node sse-load.mjs
// Env vars (all optional):
//   CHIMELY_URL        target base URL          (default http://127.0.0.1:8299)
//   CHIMELY_ENV        environment slug         (default demo)
//   CHIMELY_API_KEY    management bearer key    (default dev-secret-key)
//   N                  total SSE connections    (default 500)
//   M                  distinct subscribers     (default ceil(N / CAP))
//   CAP                per-subscriber cap       (default 8, must match server)
//   T                  hold seconds after ramp  (default 90)
//   PROBE_INTERVAL_MS  probe cadence            (default 2000; keep it above
//                      CHIMELY_HINT_DEBOUNCE_MS, default 1000, or the debounce
//                      coalesces probes and latency numbers lie)
//   RAMP_CONCURRENCY   parallel connection opens (default 100)
//   SERVER_PID         pid to sample RSS from via `ps` (default: ask the
//                      target's /metrics + lsof is NOT attempted; unset = skip)
//   SUB_PREFIX         subscriber id prefix     (default bench-sub)
//   OUT_JSON           path to write the JSON summary (default: stdout only)
//
// Exit code 0 even when connections fail: failures are data, not errors. The
// summary is the product.

import { execFileSync } from 'node:child_process';
import http from 'node:http';
import https from 'node:https';

const cfg = {
  url: new URL(process.env.CHIMELY_URL || 'http://127.0.0.1:8299'),
  env: process.env.CHIMELY_ENV || 'demo',
  apiKey: process.env.CHIMELY_API_KEY || 'dev-secret-key',
  n: parseInt(process.env.N || '500', 10),
  cap: parseInt(process.env.CAP || '8', 10),
  t: parseInt(process.env.T || '90', 10),
  probeIntervalMs: parseInt(process.env.PROBE_INTERVAL_MS || '2000', 10),
  rampConcurrency: parseInt(process.env.RAMP_CONCURRENCY || '100', 10),
  serverPid: process.env.SERVER_PID ? parseInt(process.env.SERVER_PID, 10) : null,
  subPrefix: process.env.SUB_PREFIX || 'bench-sub',
  outJson: process.env.OUT_JSON || null,
};
cfg.m = parseInt(process.env.M || String(Math.ceil(cfg.n / cfg.cap)), 10);

if (cfg.n / cfg.m > cfg.cap) {
  console.error(
    `N/M = ${(cfg.n / cfg.m).toFixed(2)} exceeds per-subscriber cap ${cfg.cap}; raise M`,
  );
  process.exit(2);
}

const transport = cfg.url.protocol === 'https:' ? https : http;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

const state = {
  established: 0,
  peakEstablished: 0,
  open: 0,
  failures: new Map(), // cause -> count
  hintLatenciesMs: [], // one sample per (probe, connection) observation
  probesSent: 0,
  probesAccepted: 0,
  probeErrors: 0,
  rssSamplesKb: [], // {t, kb}
  connectLatenciesMs: [], // time to 200 + headers per connection
  eventsSeen: 0,
  droppedMidStream: 0,
};

function fail(cause) {
  state.failures.set(cause, (state.failures.get(cause) || 0) + 1);
}

// Outstanding probe the next hint on a probe-subscriber connection attributes
// to. Probes are spaced above the server's debounce window so exactly one
// hint per connection per probe is expected.
let currentProbe = null; // {acceptedAt, seenOn: Set<connId>}

// ---------------------------------------------------------------------------
// SSE connection
// ---------------------------------------------------------------------------

function subscriberFor(connIdx) {
  // Round-robin so each subscriber holds at most ceil(N/M) <= CAP streams.
  return `${cfg.subPrefix}-${connIdx % cfg.m}`;
}

const PROBE_SUB = `${cfg.subPrefix}-0`;

function openSse(connIdx) {
  return new Promise((resolve) => {
    const sub = subscriberFor(connIdx);
    const isProbeConn = sub === PROBE_SUB;
    const started = performance.now();
    const req = transport.request(
      {
        host: cfg.url.hostname,
        port: cfg.url.port || (cfg.url.protocol === 'https:' ? 443 : 80),
        path: '/v1/inbox/stream',
        method: 'GET',
        agent: false, // dedicated socket per stream
        headers: {
          accept: 'text/event-stream',
          'x-chimely-environment': cfg.env,
          'x-chimely-subscriber': sub,
        },
      },
      (res) => {
        if (res.statusCode !== 200) {
          fail(`http_${res.statusCode}`);
          res.resume();
          res.on('end', () => resolve(false));
          res.on('error', () => resolve(false));
          return;
        }
        state.established += 1;
        state.open += 1;
        state.peakEstablished = Math.max(state.peakEstablished, state.open);
        state.connectLatenciesMs.push(performance.now() - started);
        resolve(true);

        let buf = '';
        res.setEncoding('utf8');
        res.on('data', (chunk) => {
          buf += chunk;
          let idx = buf.indexOf('\n\n');
          while (idx !== -1) {
            const frame = buf.slice(0, idx);
            buf = buf.slice(idx + 2);
            handleFrame(frame, connIdx, isProbeConn);
            idx = buf.indexOf('\n\n');
          }
          // Comment keep-alives (`: ping`) parse as frames too; harmless.
          if (buf.length > 65536) buf = ''; // never happens per contract; guard anyway
        });
        const gone = (cause) => () => {
          state.open -= 1;
          if (!shuttingDown) {
            state.droppedMidStream += 1;
            fail(cause);
          }
        };
        res.on('end', gone('server_closed_stream'));
        res.on('error', gone('stream_error'));
      },
    );
    req.on('error', (err) => {
      fail(`connect_${err.code || err.message}`);
      resolve(false);
    });
    req.end();
  });
}

function handleFrame(frame, connIdx, isProbeConn) {
  let event = 'message';
  for (const line of frame.split('\n')) {
    if (line.startsWith('event:')) event = line.slice(6).trim();
  }
  if (event === 'hint') {
    state.eventsSeen += 1;
    if (isProbeConn && currentProbe && !currentProbe.seenOn.has(connIdx)) {
      currentProbe.seenOn.add(connIdx);
      // Clamp at 0: the outbox worker can publish between the server's txn
      // commit and the POST response flushing back to us.
      state.hintLatenciesMs.push(Math.max(0, performance.now() - currentProbe.acceptedAt));
    }
  }
}

// ---------------------------------------------------------------------------
// Probe: POST /v1/notifications to the probe subscriber
// ---------------------------------------------------------------------------

function postNotification() {
  return new Promise((resolve) => {
    const body = JSON.stringify({
      subscriber_id: PROBE_SUB,
      category: 'bench.probe',
      payload: { sent_at_ms: Date.now() },
    });
    const req = transport.request(
      {
        host: cfg.url.hostname,
        port: cfg.url.port || (cfg.url.protocol === 'https:' ? 443 : 80),
        path: '/v1/notifications',
        method: 'POST',
        agent: false,
        headers: {
          authorization: `Bearer ${cfg.apiKey}`,
          'content-type': 'application/json',
          'content-length': Buffer.byteLength(body),
        },
      },
      (res) => {
        res.resume();
        res.on('error', () => {
          state.probeErrors += 1;
          fail('probe_stream_error');
          resolve();
        });
        res.on('end', () => {
          if (res.statusCode === 201 || res.statusCode === 200) {
            state.probesAccepted += 1;
            currentProbe = { acceptedAt: performance.now(), seenOn: new Set() };
          } else {
            state.probeErrors += 1;
            fail(`probe_http_${res.statusCode}`);
          }
          resolve();
        });
      },
    );
    req.on('error', () => {
      state.probeErrors += 1;
      fail('probe_connect_error');
      resolve();
    });
    req.end(body);
  });
}

// ---------------------------------------------------------------------------
// RSS sampling
// ---------------------------------------------------------------------------

function sampleRss() {
  if (!cfg.serverPid) return;
  try {
    const out = execFileSync('ps', ['-o', 'rss=', '-p', String(cfg.serverPid)], {
      encoding: 'utf8',
    });
    const kb = parseInt(out.trim(), 10);
    if (Number.isFinite(kb)) state.rssSamplesKb.push({ t: Date.now(), kb });
  } catch {
    // server gone; the hold loop will notice via dropped streams
  }
}

// ---------------------------------------------------------------------------
// Percentiles
// ---------------------------------------------------------------------------

function pct(sorted, p) {
  if (sorted.length === 0) return null;
  const i = Math.min(sorted.length - 1, Math.ceil((p / 100) * sorted.length) - 1);
  return sorted[Math.max(0, i)];
}

function summarize(arr) {
  const s = [...arr].sort((a, b) => a - b);
  return {
    count: s.length,
    p50: pct(s, 50),
    p95: pct(s, 95),
    p99: pct(s, 99),
    max: s.length ? s[s.length - 1] : null,
  };
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

let shuttingDown = false;

async function main() {
  console.error(
    `target=${cfg.url.origin} env=${cfg.env} N=${cfg.n} M=${cfg.m} cap=${cfg.cap} hold=${cfg.t}s probe_every=${cfg.probeIntervalMs}ms`,
  );

  // Ramp: open N connections with bounded parallelism.
  const rampStart = performance.now();
  let next = 0;
  async function opener() {
    while (next < cfg.n) {
      const idx = next++;
      await openSse(idx);
    }
  }
  await Promise.all(Array.from({ length: Math.min(cfg.rampConcurrency, cfg.n) }, opener));
  const rampSecs = (performance.now() - rampStart) / 1000;
  console.error(
    `ramp done in ${rampSecs.toFixed(1)}s: established=${state.established}/${cfg.n} open=${state.open}`,
  );

  // Hold + probe.
  sampleRss();
  const holdStart = Date.now();
  const probeTimer = setInterval(() => {
    state.probesSent += 1;
    postNotification();
  }, cfg.probeIntervalMs);
  const rssTimer = setInterval(sampleRss, 5000);
  const statusTimer = setInterval(() => {
    const rss = state.rssSamplesKb.at(-1);
    console.error(
      `t+${Math.round((Date.now() - holdStart) / 1000)}s open=${state.open} hints=${state.hintLatenciesMs.length} probes=${state.probesAccepted}/${state.probesSent} rss=${rss ? `${(rss.kb / 1024).toFixed(1)}MB` : 'n/a'}`,
    );
  }, 10000);

  await new Promise((r) => setTimeout(r, cfg.t * 1000));

  clearInterval(probeTimer);
  clearInterval(rssTimer);
  clearInterval(statusTimer);
  // Let the last probe's hints land.
  await new Promise((r) => setTimeout(r, 1500));
  sampleRss();

  shuttingDown = true;
  const openAtEnd = state.open;

  const rssKb = state.rssSamplesKb.map((s) => s.kb);
  const summary = {
    config: {
      target: cfg.url.origin,
      environment: cfg.env,
      n: cfg.n,
      m: cfg.m,
      cap: cfg.cap,
      hold_secs: cfg.t,
      probe_interval_ms: cfg.probeIntervalMs,
    },
    ramp_secs: Number(rampSecs.toFixed(1)),
    established: state.established,
    open_at_end: openAtEnd,
    dropped_mid_stream: state.droppedMidStream,
    failures: Object.fromEntries(state.failures),
    connect_ms: summarize(state.connectLatenciesMs),
    probes: {
      sent: state.probesSent,
      accepted: state.probesAccepted,
      errors: state.probeErrors,
    },
    hint_latency_ms: summarize(state.hintLatenciesMs),
    hint_events_total: state.eventsSeen,
    server_rss_mb: rssKb.length
      ? {
          start: Number((rssKb[0] / 1024).toFixed(1)),
          max: Number((Math.max(...rssKb) / 1024).toFixed(1)),
          end: Number((rssKb.at(-1) / 1024).toFixed(1)),
        }
      : null,
  };

  console.log(JSON.stringify(summary, null, 2));
  if (cfg.outJson) {
    const { writeFileSync } = await import('node:fs');
    writeFileSync(cfg.outJson, `${JSON.stringify(summary, null, 2)}\n`);
  }
  // Sockets are still open; exit hard rather than waiting for them to drain.
  process.exit(0);
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
