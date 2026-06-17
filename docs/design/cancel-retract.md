# Cancel and retract — design note (risk M1)

> Phase 2 deliverable (`specs/phase-2-sdk.md`, item 4). Design only. The API
> shape is settled here while the SDK surface is fresh; implementation ships
> early Phase 3 — see "Why implementation waits" at the end. Everything
> below is additive: nothing in the frozen v1 contract changes, options
> bags and optional-member discipline apply as everywhere else.

## The gap

No undo exists anywhere in v1: a scheduled notification cannot be cancelled
before `deliver_at`, a broadcast cannot be retracted. The risk (M1) is that
the first wrong announcement under real traffic turns "we cannot take it
back" into a support incident, and an undo designed in a hurry would get
the cancel-vs-deliver race wrong. This note settles the shape and the race
on paper.

## Additive API

Both endpoints are management-plane (Bearer API key), mirroring the create
endpoints they undo.

### `DELETE /v1/notifications/{notification_id}` — cancel a scheduled notification

Cancels a notification that is not yet visible (`deliver_at` in the
future). Counters never bumped for it (they bump at delivery) and no
subscriber ever saw it, so a successful cancel erases it completely.

| Outcome | Status |
|---|---|
| Cancelled (or already cancelled — see idempotency below) | `204` |
| Lost the race: the notification already delivered | `409 Conflict`, error code `already_delivered` |
| Unknown id in this environment | `404` |

### `DELETE /v1/broadcasts/{broadcast_id}` — retract a broadcast

Broadcasts are fan-out-on-read: one row per announcement, never
materialized per subscriber. Row deletion IS the retraction, O(1) by the
same construction that makes creation O(1):

- The merged list stops including it at the next read.
- Read-state exception rows (`broadcast_reads`) for the retracted
  broadcast are deleted in the same transaction.
- Counters recompute from Postgres: the unread count's broadcast
  component is a read-time range count (maintained direct counter plus
  broadcasts range count minus exception rows), so no per-subscriber
  counter row is ever touched. Redis-cached counters are recomputable
  from Postgres at any moment, per the standing invariant.

Responses: `204` (retracted, idempotently), `404` (unknown id), `401`.

Both endpoints enqueue a `hint` outbox job in the same transaction
(transactional outbox, never a dual write), so connected inboxes refetch
and the item disappears live.

## The cancel-vs-deliver race

The spec sentence this note exists to honor: a cancel must either delete
the pending deliver job or lose to it atomically — job-row deletion as the
linearization point.

Mechanics:

1. Cancel opens a transaction and issues
   `DELETE FROM jobs WHERE environment_id = $1 AND id = $2 RETURNING id`
   on the pending deliver job, then deletes the notification row(s) in the
   same transaction.
2. The worker claims jobs `FOR UPDATE SKIP LOCKED` and deletes the job row
   in the SAME transaction that makes the notification visible and bumps
   counters (jobs are deleted on completion, never status-flagged).
3. The interleavings:
   - **Job row present, unclaimed.** Cancel's DELETE returns one row: the
     delivery can never happen. Cancel wins, `204`.
   - **Job row claimed, worker mid-transaction.** Cancel's DELETE blocks
     on the row lock until the worker commits. Then it deletes zero rows:
     cancel lost, the delivery stands, `409`.
   - **Job row absent.** Delivery already completed; `409`.

   One row deleted means cancelled; zero rows means lost. There is no
   partial outcome, because the job-row delete and the notification-row
   delete commit together.

**Idempotency.** A repeated cancel of an already-cancelled notification
finds no job row AND no notification row: that distinguishes it from a
lost race (no job row, notification visible) and returns `204` again.
Cancel is safely retryable.

**Batch nuance.** Large fan-outs run as one chunked deliver job
(`progress_cursor`), shared across recipients, so a per-notification
cancel cannot use the shared job row as its arbiter. There the
notification row itself is the secondary linearization point: the worker's
chunk transaction locks the rows it delivers, and cancel's
`DELETE … WHERE id = $2 AND visible_at > now()` serializes against that
lock with the same three interleavings. Whole-batch cancellation (by
idempotency key) would use the job row exactly as above; its endpoint
shape is deliberately not settled here.

## ETag correctness for retraction

The list ETag's broadcast input is the environment's LATEST broadcast
`(created_at, id)`. Retracting the latest broadcast moves that input;
retracting an older one does not, so a conditional refetch could 304
against a list that still contains the retracted row.

Fix, alongside the retraction implementation: replace that ETag input with
an `environments.broadcasts_changed_at` timestamp bumped by broadcast
create AND delete. With create-only bumping it is exactly equivalent to
the current input, so the refactor can land ahead of the endpoints as a
no-op.

## Decided against for now

- **Soft-delete / tombstones.** Nothing reads them. The idempotency
  snapshot of the original create stays untouched: a replayed create after
  a cancel returns the original response, which acknowledged-and-dropped
  semantics already cover.
- **Subscriber-plane deletion.** End users mark read; only the management
  plane unsends.
- **Editing payloads in place.** Retract-and-resend is the model. In-place
  edits introduce version skew across already-rendered inboxes for no
  launch-relevant gain.
