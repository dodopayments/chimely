# Phase 2 — Client SDKs and `<Inbox />`

> Derived from `docs/dronte-project-plan.md` (Phases, "Phase 2"; "DX
> contract"; "Deploy-time thundering herd") and `docs/risk-register.md`
> (M1, M4). Self-contained: restates every invariant it touches.
> `specs/sdk-api.d.ts` is the frozen public surface — additive-only from
> the `contract-v1` tag (M4's pre-publish design review was executed at the
> freeze). Parallelizable with late Phase 1.

## Goal

Ship `@dronte/client` (framework-agnostic headless core) and
`@dronte/react` (hooks + the styled-but-overrideable `<Inbox />`)
implementing `specs/sdk-api.d.ts` exactly, plus the Next.js example that
doubles as the live demo. Close the contract loop: the oasdiff delta
reaches zero and the CI `contract` job flips from allowed-to-fail to
required.

## Deliverables

1. **`@dronte/client`.** Everything declared in the `'@dronte/client'`
   module of specs/sdk-api.d.ts: `DronteClient` (connect/close,
   getSnapshot/subscribe, fetchMore/refresh, markRead/markAllRead/
   markAllSeen, get/setPreferences), the inbox store, SSE
   connect/reconnect/resume, auth headers (and query fallbacks for
   EventSource), keyset pagination, optimistic updates, `DronteError`,
   `BackoffConfig` defaults as documented. Wire types come from
   `src/generated/` (produced by `pnpm generate` from the exported spec —
   never hand-edit).
2. **`@dronte/react`.** `DronteProvider`/`useDronteClient`,
   `useNotifications`, `useUnreadCount`, `useUnseenCount`,
   `usePreferences`, and `<Inbox />` (bell + badge + popover list +
   infinite scroll + per-category preference panel) with `InboxAppearance`
   CSS variables, slot classNames, localization, placement, and the
   render-prop overrides — exactly the props frozen in specs/sdk-api.d.ts.
3. **Next.js example** (`examples/`): quickstart app that is also the
   public live demo. A Vite example and an axum trigger example follow the
   plan's deliverables list as stretch.
4. **Cancel/retract design note (risk M1).** No undo exists anywhere: no
   cancel for scheduled notifications, no broadcast retraction, no delete.
   Design the additive endpoints and think through the cancel-vs-deliver-
   job race (a cancel must either delete the pending `deliver` job or lose
   to it atomically — job-row deletion as the linearization point).
   Design doc in this phase; implementation may ship here or early
   Phase 3, but the API shape must be settled while the SDK is fresh.
5. **Release plumbing.** Changesets-driven versioned publish of both
   packages at v0.1; semver discipline from then on.
6. **Contract flip.** When the oasdiff delta is empty: remove
   `continue-on-error: true` from the `contract` CI job and mark it
   required in branch protection (the Phase 1/2 completion criterion).

## Invariants in play (restated; violating any is a bug)

- **SSE events are HINTS, not transports.** Every hint AND every
  (re)connect triggers a conditional (ETag/If-None-Match) REST refetch of
  page one + counts. Missed hints are harmless; a 304 costs nothing. The
  client never renders from event payloads.
- **Jittered exponential backoff is not optional in spirit** — it is the
  deploy-time thundering-herd protection. Defaults: initial 1000ms, max
  30000ms, multiplier 2, jitter 0.5, maxAttempts Infinity. The server's
  graceful-close `retry:` directive overrides the next delay. N clients
  dropped by a restart must not reconnect in lockstep.
- **`EventSourceLike` 'error' events drive the reconnect loop** — an
  implementation that never emits them silently breaks reconnection (this
  is why the structural type requires them).
- **All mutations are optimistic**: the snapshot updates synchronously,
  the server call follows, a failure rolls back and surfaces on the error
  channel. `InboxSnapshot` is immutable — new object identity on every
  change (`useSyncExternalStore`-safe).
- **markAllSeen is the bell-open gesture**: opening the popover zeroes
  `unseen` without touching read state. `unread` drives list styling;
  `unseen` drives the badge.
- **Payloads are wire format**: snake_case keys pass through verbatim,
  never case-transformed; unknown fields ride along for custom renderers;
  `body` is plain text, never HTML. IDs are opaque TypeIDs whose prefix
  encodes the source; `source` stays the explicit discriminator routing
  mark-read.
- **Preference absence means enabled** — the SDK only ever holds explicit
  rows; setting enabled=true deletes the explicit row server-side.
- **Additive-only surface.** Nothing in specs/sdk-api.d.ts is removed,
  renamed, or narrowed in a minor; new members are optional with safe
  defaults; options bags, never positional params; timestamps are RFC 3339
  strings, never Date. `InboxLocalization` has no index signature on
  purpose (typos must not type-check).
- **Styling.** Plain CSS with custom properties (`--dronte-*` forwarded
  verbatim), slot classNames, render props. `@floating-ui/dom` is the ONLY
  runtime UI dependency; no Tailwind in published packages.
- **Generated types are generated.** `packages/client/src/generated/` and
  `docs/openapi/` come from `pnpm generate`; CI fails if stale.

## Out of scope (deliberately)

Vue/Svelte bindings (community, on top of `@dronte/client`). JWT widget
auth (v2). Web-push service-worker story (Phase 6+). Digest/batching.

## Acceptance criteria

- [ ] **oasdiff equivalence reached**: `oasdiff diff specs/openapi.yaml
      <(cargo run -- openapi)` is EMPTY, and the `contract` job is flipped
      to required (continue-on-error removed) in the same PR that empties
      the diff.
- [ ] Type-level conformance test: the built `.d.ts` of each package
      satisfies the corresponding module declaration in
      specs/sdk-api.d.ts (compile-time assertion in CI, not review-time
      diligence).
- [ ] Reconnect behavior: a dropped stream reconnects with jittered
      exponential delays (statistical test over many simulated drops — no
      lockstep), resumes with Last-Event-ID, and refetches conditionally
      on every (re)connect (mostly 304s after a deploy-style mass drop).
- [ ] Optimistic rollback: a failed markRead/markAllRead/setPreferences
      restores the previous snapshot and surfaces a `DronteError` with the
      server's error code; a successful one never flickers.
- [ ] `<Inbox />` renders with zero styling dependencies; appearance
      variables and slot classNames apply; each render prop fully replaces
      its slot; popover-open calls markAllSeen; infinite scroll pages via
      fetchMore; `preferencesPanel={false}` hides the panel.
- [ ] Default item click marks read then follows `payload.action_url`
      unless `onItemClick` returns false.
- [ ] Next.js example runs against a local dronte (Redis-less mode) as a
      30-second quickstart: one `POST /v1/notifications` curl appears live
      in the example's inbox.
- [ ] Both packages publish via changesets (dry-run in CI), ESM+CJS+types,
      `sideEffects: false`; vitest green; Biome and typecheck green.
- [ ] Cancel/retract design note reviewed and committed (docs/ or specs/
      addendum), with the cancel-vs-deliver-job race resolved on paper.
