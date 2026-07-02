/**
 * Domain types of the public SDK surface, stable and additive-only. Wire
 * shapes live in ./generated/api.d.ts and are mapped at the fetch boundary.
 * Payloads are the exception. They are wire format, passed through verbatim.
 */

import type { ChimelyError } from './errors';

export type InboxItemSource = 'notification' | 'broadcast';

/** TypeID of a direct notification: `notif_` + UUIDv7 in Crockford base32. */
export type NotificationId = `notif_${string}`;
/** TypeID of a broadcast: `bcast_` + UUIDv7 in Crockford base32. */
export type BroadcastId = `bcast_${string}`;
export type InboxItemId = NotificationId | BroadcastId;

/**
 * The payload convention the default <Inbox /> rendering understands.
 * All fields optional. Payloads are customer-defined and pass through
 * Chimely verbatim (snake_case keys, never case-transformed by the SDK).
 * Unknown fields ride along for custom renderers. This interface only
 * ever gains optional fields.
 */
export interface WellKnownPayload {
  /** First line of the default item rendering. */
  title?: string;
  /** Secondary line. Treated as plain text, never HTML. */
  body?: string;
  /** Followed on item click by the default renderer (after mark-read). */
  action_url?: string;
  /** Leading icon/avatar in the default rendering. */
  icon_url?: string;
  [custom: string]: unknown;
}

/**
 * One merged-inbox entry. `TPayload` lets apps type their own payloads.
 * Chimely never interprets payloads.
 */
export interface InboxItem<TPayload = WellKnownPayload> {
  /** The TypeID prefix encodes the source. `source` is the ergonomic discriminator. */
  id: InboxItemId;
  /** Source table. The client routes mark-read with this. */
  source: InboxItemSource;
  /** Customer-defined category, e.g. `payment.failed`. Drives rendering. */
  category: string;
  payload: TPayload;
  /** Ordering timestamp (RFC 3339). visible_at for direct, created_at for broadcast. */
  occurredAt: string;
  read: boolean;
}

export interface InboxCounts {
  /** Items not yet read. Drives list styling. */
  unread: number;
  /** Items newer than the seen watermark. Drives the bell badge. */
  unseen: number;
}

export interface Preference {
  category: string;
  /** Only 'in_app' exists in v1. The union widens (never narrows) when push lands. */
  channel: 'in_app';
  enabled: boolean;
}

export type ConnectionStatus =
  | 'idle' // constructed, connect() not yet called
  | 'connecting' // first SSE attempt in flight
  | 'connected' // live stream
  | 'reconnecting' // backoff loop after a drop, REST still works
  | 'closed'; // close() called, terminal until connect()

/**
 * Jittered exponential backoff for SSE reconnects. Jitter is the deploy-time
 * thundering-herd protection: N clients dropped by a restart must not
 * reconnect in lockstep. The server's graceful-close `retry:` directive,
 * when present, overrides the next delay.
 */
export interface BackoffConfig {
  /** First retry delay. Default: 1000. */
  initialDelayMs?: number;
  /** Delay ceiling. Default: 30000. */
  maxDelayMs?: number;
  /** Exponential multiplier. Default: 2. */
  multiplier?: number;
  /** Randomization factor 0..1 applied as ±(jitter × delay). Default: 0.5. */
  jitter?: number;
  /**
   * Give up after this many consecutive failures (status becomes 'closed',
   * an 'error' event fires). Default: Infinity.
   */
  maxAttempts?: number;
}

export interface ChimelyClientConfig {
  /** Chimely server origin, e.g. `https://chimely.dev`. */
  serverUrl: string;
  /** Environment slug, e.g. `dashboard-prod`. */
  environment: string;
  /** Customer-provided subscriber id of the current user. */
  subscriberId: string;
  /**
   * HMAC-SHA256(secret, subscriberId) hex, computed by YOUR backend.
   * Required in production environments. Omittable only where the
   * environment allows it (dev quickstart).
   */
  subscriberHash?: string;
  backoff?: BackoffConfig;
  /** Page size for list fetches (1–100). Default: 20. */
  pageSize?: number;
  /** Custom fetch (SSR, testing, instrumentation). Default: globalThis.fetch. */
  fetchFn?: typeof fetch;
  /**
   * EventSource factory (polyfills, React Native, testing). The default
   * uses the platform EventSource. The reconnect loop recreates the source,
   * so the resume token also rides the stream URL as `last_event_id`.
   */
  createEventSource?: (url: string) => EventSourceLike;
}

/**
 * Minimal structural EventSource so non-browser runtimes can plug in.
 * The client listens for named hint events plus 'open' and 'error'.
 * 'error' events carry no data but are what drive the reconnect/backoff
 * loop, so an implementation that never emits them silently breaks
 * reconnection. On graceful shutdown the server also sends a named
 * 'retry' event whose data is the next delay in milliseconds, which any
 * EventSource delivers like a hint event.
 */
export interface EventSourceLike {
  addEventListener(
    type: 'open' | 'error' | string,
    listener: (event: { data?: string; lastEventId?: string }) => void,
  ): void;
  close(): void;
}

/**
 * Immutable snapshot of everything the UI needs. New object identity on
 * every change. Safe for `useSyncExternalStore` and equivalents.
 */
export interface InboxSnapshot<TPayload = WellKnownPayload> {
  items: ReadonlyArray<InboxItem<TPayload>>;
  counts: InboxCounts;
  status: ConnectionStatus;
  /** False once the last page has been fetched. */
  hasMore: boolean;
  /** True during the initial load and refreshes (not during fetchMore). */
  isLoading: boolean;
  /** Last unrecovered error. Cleared by the next successful operation. */
  error: ChimelyError | null;
}
