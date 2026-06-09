# Phase 4 — Admin dashboard

> Derived from `docs/dronte-project-plan.md` (Phases, "Phase 4"; stack:
> "Admin SPA"). Self-contained: restates every invariant it touches.
> 1–2 weeks — short precisely because single-org means there is no org or
> user management to build.

## Goal

An embedded admin SPA (served from the same single binary) that gives the
operator eyes and hands: browse notifications and their status timelines,
compose broadcasts, look up subscribers, replay the DLQ, and manage API
keys and environments. The single-file deploy story must survive intact.

## Deliverables

1. **SPA shell.** Vite + React + TanStack Query/Router, embedded via
   rust-embed, served by the dronte binary (e.g. under `/admin`). No
   separate deployment artifact; `docker run dronte` ships the dashboard.
2. **Instance-level admin auth.** Static credential via env var at launch
   (OIDC later). One credential for the whole instance — there are no
   admin "users". All admin endpoints and the SPA gate on it.
3. **Notification/status browser.** Filter by environment, subscriber,
   category, time window; inspect payloads; view each notification's
   append-only status timeline (`created → delivered_hint → seen → read`)
   — the "did it send?" answer.
4. **Broadcast composer.** Create broadcasts (category, payload — with the
   well-known fields given a friendly form) targeting an environment.
   Composing is creating: same idempotent management-plane semantics.
5. **Subscriber lookup.** By customer-provided subscriber_id within an
   environment: counters, both watermarks, explicit preference rows,
   recent merged inbox as the subscriber sees it (same canonical query),
   broadcast visibility window (`created_at`).
6. **DLQ replay.** List parked jobs with `last_error`/`attempts`; replay
   selected jobs; replayed jobs re-enter the normal claim path.
7. **API key + environment management.** Create environment (slug, name,
   require_subscriber_hash); create/revoke API keys (plaintext shown
   exactly once, stored as sha256 hash + display prefix); rotate the
   subscriber HMAC secret with the two-slot overlap (current + previous
   verified during rotation; previous cleared when rotation ends).
8. **Admin API surface** as utoipa-annotated endpoints in the generated
   spec (own tag). Post-v1 the contract gate is breaking-change detection,
   so additive admin endpoints pass by construction.

## Invariants in play (restated; violating any is a bug)

- **Single-org, no exceptions.** No organizations table, no user
  management, no roles. Environments are the only isolation unit;
  multi-tenancy is "run another instance". The admin plane must not grow
  org-shaped concepts (the non-goals list is product strategy).
- **Environment isolation everywhere.** Every admin query is either
  scoped to one environment_id or is an explicit, documented cross-
  environment admin path (the allowed exception in the schema's
  shard-readiness invariant #3). No query may join across environments
  implicitly.
- **Admin reads reuse the canonical queries.** The subscriber-view in the
  lookup screen runs the SAME merged-list and unread-count queries the
  subscriber plane uses (two-source merge: direct + broadcast, watermark +
  exceptions). A second, admin-only implementation of the merge is a bug
  by definition — two implementations WILL disagree.
- **Admin writes obey the same write-path rules.** Broadcast composing is
  one row (fan-out on read, never materialized per subscriber). Any
  admin-triggered read-state repair moves watermarks or per-item
  exceptions — NEVER a bulk UPDATE over notifications. DLQ replay re-
  enqueues through the normal claim path (SKIP LOCKED, per-environment
  fairness, delete-on-completion); it does not execute jobs inline.
- **Key material handling.** API keys: hash-only storage (sha256), display
  prefix for recognition, plaintext rendered once at creation and never
  retrievable; revocation is a soft `revoked_at` (rows kept for audit).
  HMAC secrets: dedicated (never an API key), two-slot rotation so live
  `<Inbox />` sessions survive rotation.
- **The binary stays one file.** rust-embed; no CDN, no separate static
  host. `/healthz`, `/readyz` semantics unchanged by the admin plane.
- **specs/ remain read-only**; admin endpoints extend the generated spec
  additively and must not motivate edits to the frozen v1 contracts.

## Out of scope (deliberately)

OIDC (later), audit log UI, member/role management (never — single-org),
notification editing or deletion (no undo primitives exist yet — cancel/
retract ships per the Phase 2 design note, and the admin UI exposes it
only once the API exists), analytics dashboards.

## Acceptance criteria

- [ ] `docker run` of the published image serves the dashboard at
      `/admin` with no extra artifacts; total deploy remains binary +
      Postgres + Redis.
- [ ] Admin auth: every admin endpoint and SPA route 401s without the
      credential; the credential is supplied only via env var; it never
      appears in logs.
- [ ] Status browser answers "did notification X send?" end to end:
      create → see timeline transitions appear live.
- [ ] Subscriber lookup shows counters, watermarks, preferences, and a
      merged inbox identical to what the subscriber-plane API returns for
      the same subscriber (golden test comparing both code paths).
- [ ] Broadcast composed in the UI lands as ONE row and appears in a
      subscriber's merged list (visibility rule respected for subscribers
      created after the broadcast).
- [ ] DLQ replay: a parked job replayed from the UI completes and its row
      is DELETED; fairness and claim-path metrics show it went through the
      normal worker loop.
- [ ] API key create/revoke round-trip; revoked key 401s immediately;
      key listing shows prefix + last_used_at, never the key.
- [ ] HMAC rotation via UI: widgets authenticated with the previous secret
      keep working during the overlap window; previous slot cleared on
      completion; rotation is observable in the environment view.
- [ ] Generated spec gains the admin tag additively; the (post-v1)
      breaking-change contract gate stays green; TS and Rust CI green.
