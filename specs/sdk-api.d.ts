/**
 * Dronte v1 — public SDK surface (contract-first spec).
 *
 * This file IS the published contract for `@dronte/client` and
 * `@dronte/react`. Stability rules (semver discipline from v0.1):
 *
 *   - Nothing declared here is removed, renamed, or narrowed in a minor.
 *   - All future additions are OPTIONAL members with safe defaults.
 *   - Every object-shaped parameter is an options bag (never positional
 *     params) so fields can be added without a major.
 *   - Timestamps cross this boundary as RFC 3339 strings, never Date —
 *     JSON-faithful, no timezone or serialization ambiguity.
 *   - IDs are TypeIDs (`notif_…`, `bcast_…`) — opaque strings whose prefix
 *     names the resource type; the template-literal types below let the
 *     compiler catch an id handed to the wrong endpoint.
 *   - Community bindings (Vue/Svelte) build on `@dronte/client` only; nothing
 *     in `@dronte/react` is load-bearing for them.
 */

// ============================================================================
// @dronte/client — framework-agnostic headless core.
// Owns: auth, REST calls, SSE connect/reconnect/resume, the inbox store,
// optimistic updates, pagination. Zero DOM/framework dependencies.
// ============================================================================
declare module '@dronte/client' {
  // -------------------------------------------------------------- domain ---

  export type InboxItemSource = 'notification' | 'broadcast';

  /** TypeID of a direct notification: `notif_` + UUIDv7 in Crockford base32. */
  export type NotificationId = `notif_${string}`;
  /** TypeID of a broadcast: `bcast_` + UUIDv7 in Crockford base32. */
  export type BroadcastId = `bcast_${string}`;
  export type InboxItemId = NotificationId | BroadcastId;

  /**
   * The payload convention the default <Inbox /> rendering understands —
   * mirrors `Payload` in the OpenAPI spec. All fields optional; payloads are
   * customer-defined and pass through Dronte verbatim (snake_case keys:
   * payloads are wire format, never case-transformed by the SDK). Unknown
   * fields ride along for custom renderers. This interface only ever gains
   * optional fields.
   */
  export interface WellKnownPayload {
    /** First line of the default item rendering. */
    title?: string;
    /** Secondary line; treated as plain text, never HTML. */
    body?: string;
    /** Followed on item click by the default renderer (after mark-read). */
    action_url?: string;
    /** Leading icon/avatar in the default rendering. */
    icon_url?: string;
    [custom: string]: unknown;
  }

  /**
   * One merged-inbox entry. `TPayload` lets apps type their own payloads
   * (per-category discrimination is the app's concern — Dronte never
   * interprets payloads).
   */
  export interface InboxItem<TPayload = WellKnownPayload> {
    /** The TypeID prefix encodes the source; `source` is the ergonomic discriminator. */
    id: InboxItemId;
    /** Which table it came from; the client routes mark-read with this. */
    source: InboxItemSource;
    /** Customer-defined category, e.g. `payment.failed`. Drives rendering. */
    category: string;
    payload: TPayload;
    /** Ordering timestamp (RFC 3339): visible_at for direct, created_at for broadcast. */
    occurredAt: string;
    read: boolean;
  }

  export interface InboxCounts {
    /** Items not yet read — drives list styling. */
    unread: number;
    /** Items newer than the seen watermark — drives the bell badge. */
    unseen: number;
  }

  export interface Preference {
    category: string;
    /** Only 'in_app' exists in v1; the union widens (never narrows) when push lands. */
    channel: 'in_app';
    enabled: boolean;
  }

  export type ConnectionStatus =
    | 'idle'          // constructed, connect() not yet called
    | 'connecting'    // first SSE attempt in flight
    | 'connected'     // live stream
    | 'reconnecting'  // backoff loop after a drop; REST still works
    | 'closed';       // close() called; terminal until connect()

  export class DronteError extends Error {
    /** Machine-readable code from the server error envelope, or a client-side code ('network', 'unauthorized', ...). */
    readonly code: string;
    /** HTTP status when the error came from the server. */
    readonly status?: number;
  }

  // -------------------------------------------------------------- config ---

  /**
   * Jittered exponential backoff for SSE reconnects. Jitter is not optional
   * in spirit: it is the deploy-time thundering-herd protection — N clients
   * dropped by a restart must not reconnect in lockstep. The server's
   * graceful-close `retry:` directive, when present, overrides the next delay.
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
     * an 'error' event fires). Default: Infinity — an inbox should outlive
     * any outage.
     */
    maxAttempts?: number;
  }

  export interface DronteClientConfig {
    /** Dronte server origin, e.g. `https://dronte.example.com`. */
    serverUrl: string;
    /** Environment slug, e.g. `dashboard-prod`. */
    environment: string;
    /** Customer-provided subscriber id of the current user. */
    subscriberId: string;
    /**
     * HMAC-SHA256(secret, subscriberId) hex, computed by YOUR backend.
     * Required in production environments; omittable only where the
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
     * uses the platform EventSource; Last-Event-ID resume is handled by the
     * platform on reconnect.
     */
    createEventSource?: (url: string) => EventSourceLike;
  }

  /** Minimal structural EventSource so non-browser runtimes can plug in. */
  export interface EventSourceLike {
    addEventListener(type: string, listener: (event: { data: string; lastEventId?: string }) => void): void;
    close(): void;
  }

  // --------------------------------------------------------------- store ---

  /**
   * Immutable snapshot of everything the UI needs. New object identity on
   * every change — safe for `useSyncExternalStore` and equivalents.
   */
  export interface InboxSnapshot<TPayload = WellKnownPayload> {
    items: ReadonlyArray<InboxItem<TPayload>>;
    counts: InboxCounts;
    status: ConnectionStatus;
    /** False once the last page has been fetched. */
    hasMore: boolean;
    /** True during the initial load and refreshes (not during fetchMore). */
    isLoading: boolean;
    /** Last unrecovered error; cleared by the next successful operation. */
    error: DronteError | null;
  }

  /**
   * The headless inbox. Lifecycle: construct → connect() → use → close().
   *
   * Behavioral contract (tested invariants, not implementation details):
   * - SSE events are HINTS: every hint and every (re)connect triggers a
   *   conditional (ETag/If-None-Match) REST refetch. Missed hints are
   *   harmless; a 304 costs nothing.
   * - All mutations are OPTIMISTIC: the snapshot updates synchronously,
   *   the server call follows, and a failure rolls back and surfaces on the
   *   'error' channel.
   * - markAllSeen() is what the bell-open gesture calls; it zeroes `unseen`
   *   without touching read state.
   */
  export class DronteClient<TPayload = WellKnownPayload> {
    constructor(config: DronteClientConfig);

    /** Open the SSE stream and load the first page + counts. Idempotent. */
    connect(): void;
    /** Close the stream and stop reconnecting. The store remains readable. */
    close(): void;

    getSnapshot(): InboxSnapshot<TPayload>;
    /** Subscribe to snapshot changes. Returns the unsubscribe function. */
    subscribe(listener: () => void): () => void;

    /** Load the next page (keyset cursor managed internally). No-op when !hasMore. */
    fetchMore(): Promise<void>;
    /** Conditional refetch of page one + counts (the hint/reconnect path). */
    refresh(): Promise<void>;

    markRead(item: { id: InboxItemId; source: InboxItemSource }): Promise<void>;
    markAllRead(): Promise<void>;
    markAllSeen(): Promise<void>;

    /** Explicit rows only — a category absent here is enabled. */
    getPreferences(): Promise<Preference[]>;
    /** Partial upsert; returns the resulting explicit rows. */
    setPreferences(preferences: Preference[]): Promise<Preference[]>;
  }
}

// ============================================================================
// @dronte/react — bindings + the styled-but-overrideable <Inbox />.
// Zero styling dependencies; CSS variables + slot classNames + render props.
// ============================================================================
declare module '@dronte/react' {
  import type { ReactNode } from 'react';
  import type {
    DronteClient,
    DronteClientConfig,
    DronteError,
    InboxItem,
    InboxItemId,
    InboxItemSource,
    Preference,
    WellKnownPayload,
  } from '@dronte/client';

  // ------------------------------------------------------------ provider ---

  /**
   * Provides one shared DronteClient to all hooks below. Pass either a
   * pre-built `client` (you own its lifecycle) or `config` (the provider
   * constructs, connects on mount, closes on unmount).
   */
  export interface DronteProviderProps {
    client?: DronteClient;
    config?: DronteClientConfig;
    children?: ReactNode;
  }
  export function DronteProvider(props: DronteProviderProps): ReactNode;

  /** The provider's client. Throws outside a <DronteProvider>. */
  export function useDronteClient(): DronteClient;

  // --------------------------------------------------------------- hooks ---

  export interface UseNotificationsOptions {
    /** Override the client's pageSize for this consumer. */
    pageSize?: number;
  }

  export interface UseNotificationsResult<TPayload = WellKnownPayload> {
    items: ReadonlyArray<InboxItem<TPayload>>;
    isLoading: boolean;
    error: DronteError | null;
    hasMore: boolean;
    fetchMore: () => Promise<void>;
    refresh: () => Promise<void>;
    markRead: (item: { id: InboxItemId; source: InboxItemSource }) => Promise<void>;
    markAllRead: () => Promise<void>;
  }

  /** Headless merged-inbox list. */
  export function useNotifications<TPayload = WellKnownPayload>(
    options?: UseNotificationsOptions
  ): UseNotificationsResult<TPayload>;

  export interface UseCountResult {
    count: number;
    isLoading: boolean;
    error: DronteError | null;
  }

  /** Live unread count (list styling / "N unread" copy). */
  export function useUnreadCount(): UseCountResult;
  /** Live unseen count (the bell badge). Cleared by markAllSeen. */
  export function useUnseenCount(): UseCountResult;

  export interface UsePreferencesResult {
    /** Explicit rows only — absence means enabled. */
    preferences: ReadonlyArray<Preference>;
    setPreferences: (preferences: Preference[]) => Promise<void>;
    isLoading: boolean;
    error: DronteError | null;
  }
  export function usePreferences(): UsePreferencesResult;

  // ------------------------------------------------------------- <Inbox> ---

  /**
   * Named slots for classNames overrides. This union only ever widens.
   */
  export type InboxSlot =
    | 'root'
    | 'bell'
    | 'badge'
    | 'popover'
    | 'header'
    | 'list'
    | 'item'
    | 'itemUnread'
    | 'empty'
    | 'footer'
    | 'preferences';

  /**
   * Theming without a styling dependency: CSS custom properties applied at
   * the root, plus per-slot class hooks. Variable names are part of the
   * contract.
   */
  export interface InboxAppearance {
    variables?: {
      colorPrimary?: string;
      colorBackground?: string;
      colorForeground?: string;
      colorMuted?: string;
      colorBadge?: string;
      borderRadius?: string;
      fontFamily?: string;
      fontSize?: string;
      /** Extension point: forwarded as `--dronte-<key>` verbatim. */
      [customProperty: string]: string | undefined;
    };
    classNames?: Partial<Record<InboxSlot, string>>;
  }

  export interface InboxLocalization {
    emptyTitle: string;
    emptyBody: string;
    markAllRead: string;
    preferencesTitle: string;
    /** Extension point for future strings. */
    [key: string]: string;
  }

  /**
   * Drop-in bell + badge + popover inbox.
   *
   * Two usage modes:
   * - Standalone: pass `serverUrl`/`environment`/`subscriberId`(/`subscriberHash`)
   *   and <Inbox /> constructs and owns its client.
   * - Provided: render inside <DronteProvider> and pass no connection props.
   *   Connection props, when present, take precedence over the provider.
   *
   * Built-in behavior (part of the contract):
   * - Opening the popover calls markAllSeen (badge clears; unread untouched).
   * - The list infinite-scrolls via fetchMore.
   * - A preferences panel (per-category in_app toggles) is included; hide it
   *   with `preferencesPanel={false}`.
   */
  export interface InboxProps<TPayload = WellKnownPayload> {
    // -- connection (standalone mode; all-or-nothing) --
    serverUrl?: string;
    environment?: string;
    subscriberId?: string;
    subscriberHash?: string;
    backoff?: DronteClientConfig['backoff'];

    // -- appearance --
    appearance?: InboxAppearance;
    localization?: Partial<InboxLocalization>;
    /** Popover placement relative to the bell. Default: 'bottom-end'. */
    placement?: 'bottom-start' | 'bottom-end' | 'top-start' | 'top-end';
    /** Show the per-category preferences panel. Default: true. */
    preferencesPanel?: boolean;

    // -- behavior --
    /**
     * Item click handler. Default behavior (markRead + follow
     * `payload.action_url` if present — see WellKnownPayload) runs unless
     * this returns false.
     */
    onItemClick?: (item: InboxItem<TPayload>) => boolean | void;

    // -- render props (each fully replaces its slot's default rendering) --
    renderItem?: (ctx: {
      item: InboxItem<TPayload>;
      markRead: () => Promise<void>;
    }) => ReactNode;
    renderBell?: (ctx: { unseenCount: number; open: boolean }) => ReactNode;
    renderEmpty?: () => ReactNode;
  }

  export function Inbox<TPayload = WellKnownPayload>(
    props: InboxProps<TPayload>
  ): ReactNode;
}
