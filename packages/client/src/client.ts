import type { ResolvedBackoff } from './backoff';
import { backoffDelayMs, resolveBackoff } from './backoff';
import { ChimelyError, errorFromResponse, networkError } from './errors';
import type { components } from './generated/api';
import { InboxStore } from './store';
import type {
  ChimelyClientConfig,
  EventSourceLike,
  InboxFilterView,
  InboxItem,
  InboxItemId,
  InboxItemSource,
  InboxSnapshot,
  Preference,
  WellKnownPayload,
} from './types';

type WireInboxItem = components['schemas']['InboxItem'];
type WireInboxPage = components['schemas']['InboxPage'];
type WireCounts = components['schemas']['InboxCounts'];
type WirePreferenceList = components['schemas']['PreferenceList'];

function toItem<TPayload>(wire: WireInboxItem): InboxItem<TPayload> {
  return {
    id: wire.id as InboxItemId,
    source: wire.source,
    category: wire.category,
    // Payloads pass through verbatim, never case-transformed.
    payload: wire.payload as TPayload,
    occurredAt: wire.occurred_at,
    read: wire.read,
    archived: wire.archived,
  };
}

/**
 * List order is (occurred_at, id) descending across both sources. RFC 3339
 * UTC timestamps and UUIDv7-suffixed TypeIDs both compare correctly as
 * strings within one source of generation.
 */
function isOlderThan<T>(item: InboxItem<T>, boundary: InboxItem<T>): boolean {
  if (item.occurredAt !== boundary.occurredAt) {
    return item.occurredAt < boundary.occurredAt;
  }
  return item.id < boundary.id;
}

function asChimelyError(cause: unknown): ChimelyError {
  return cause instanceof ChimelyError ? cause : networkError(cause);
}

const defaultCreateEventSource = (url: string): EventSourceLike => {
  const Ctor = (globalThis as { EventSource?: new (url: string) => EventSourceLike }).EventSource;
  if (!Ctor) {
    throw new ChimelyError(
      'no EventSource implementation available, pass createEventSource in the client config',
      { code: 'no_event_source' },
    );
  }
  return new Ctor(url);
};

/**
 * The headless inbox. Lifecycle: construct, connect(), use, close().
 *
 * - SSE events are hints. Every hint and every (re)connect triggers a
 *   conditional (ETag/If-None-Match) REST refetch. Missed hints are
 *   harmless. The client never renders from event payloads.
 * - Mutations are optimistic. The snapshot updates synchronously, the
 *   server call follows, and a failure rolls back and surfaces on the
 *   'error' channel. Void mutations resolve either way. Calls that return
 *   data (getPreferences, setPreferences) also reject with the ChimelyError.
 * - markAllSeen() zeroes unseen without touching read state.
 */
export class ChimelyClient<TPayload = WellKnownPayload> {
  private readonly config: ChimelyClientConfig;
  private readonly baseUrl: string;
  private readonly pageSize: number;
  private readonly backoff: ResolvedBackoff;
  private readonly fetchFn: typeof fetch;
  private readonly createSource: (url: string) => EventSourceLike;
  private readonly store = new InboxStore<TPayload>();

  private source: EventSourceLike | null = null;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  /** Consecutive stream failures since the last successful open. */
  private attempts = 0;
  /** Bumped by connect() and close() so stale stream callbacks become no-ops. */
  private generation = 0;
  /** Resume token from the last seen event, replayed as `last_event_id` on reconnect. */
  private lastEventId: string | null = null;
  /** One-shot delay override from the server's graceful-close retry directive. */
  private retryOverrideMs: number | null = null;

  /** Validator of the last 200 first-page response, sent as If-None-Match. */
  private etag: string | null = null;
  /** Keyset cursor of the deepest fetched page. */
  private cursor: string | null = null;

  private refreshing: Promise<void> | null = null;
  private refreshAgain = false;
  private fetchingMore: Promise<void> | null = null;

  constructor(config: ChimelyClientConfig) {
    this.config = config;
    this.baseUrl = config.serverUrl.replace(/\/+$/, '');
    this.pageSize = Math.min(100, Math.max(1, config.pageSize ?? 20));
    this.backoff = resolveBackoff(config.backoff);
    this.fetchFn = config.fetchFn ?? ((...args: Parameters<typeof fetch>) => fetch(...args));
    this.createSource = config.createEventSource ?? defaultCreateEventSource;
  }

  /** Open the SSE stream and load the first page + counts. Idempotent. */
  connect(): void {
    const { status } = this.store.getSnapshot();
    if (status !== 'idle' && status !== 'closed') {
      return;
    }
    this.generation += 1;
    this.attempts = 0;
    this.store.patch({ status: 'connecting' });
    this.openStream();
    void this.refresh();
  }

  /** Close the stream and stop reconnecting. The store remains readable. */
  close(): void {
    this.generation += 1;
    if (this.reconnectTimer !== null) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    this.source?.close();
    this.source = null;
    this.store.patch({ status: 'closed' });
  }

  getSnapshot(): InboxSnapshot<TPayload> {
    return this.store.getSnapshot();
  }

  /** Subscribe to snapshot changes. Returns the unsubscribe function. */
  subscribe(listener: () => void): () => void {
    return this.store.subscribe(listener);
  }

  /**
   * Conditional refetch of page one + counts (the hint/reconnect path).
   * Concurrent calls coalesce: a request arriving mid-flight queues exactly
   * one rerun so the freshest change is never skipped.
   */
  refresh(): Promise<void> {
    if (this.refreshing) {
      this.refreshAgain = true;
      return this.refreshing;
    }
    this.refreshing = (async () => {
      try {
        do {
          this.refreshAgain = false;
          await this.doRefresh();
        } while (this.refreshAgain);
      } finally {
        // Cleared inside the body, not via promise.finally. A finally
        // handler leaves a one-microtask gap where refreshing is non-null
        // but the rerun loop has exited, dropping a coalesce request.
        this.refreshing = null;
      }
    })();
    return this.refreshing;
  }

  /** Load the next page (keyset cursor managed internally). No-op when !hasMore. */
  fetchMore(options?: { limit?: number }): Promise<void> {
    if (!this.store.getSnapshot().hasMore) {
      return Promise.resolve();
    }
    if (this.fetchingMore) {
      return this.fetchingMore;
    }
    this.fetchingMore = this.doFetchMore(options?.limit).finally(() => {
      this.fetchingMore = null;
    });
    return this.fetchingMore;
  }

  /**
   * Switch the server-side list view. Resets pagination and the ETag (the
   * server keys validators per view) and refetches. No-op when unchanged.
   */
  setFilter(filter: InboxFilterView): Promise<void> {
    if ((this.store.getSnapshot().filter ?? 'default') === filter) {
      return Promise.resolve();
    }
    this.cursor = null;
    this.etag = null;
    this.store.patch({
      filter,
      items: [],
      hasMore: true,
      lastRefreshNewItemIds: [],
    });
    return this.refresh();
  }

  /** The `filter` query parameter for the active view, or empty. */
  private filterParam(): string {
    const filter = this.store.getSnapshot().filter ?? 'default';
    return filter === 'default' ? '' : `&filter=${filter}`;
  }

  async markRead(item: { id: InboxItemId; source: InboxItemSource }): Promise<void> {
    const prev = this.store.getSnapshot();
    const target = prev.items.find((candidate) => candidate.id === item.id);
    const changed = target !== undefined && !target.read;
    if (changed) {
      this.store.patch({
        items: prev.items.map((candidate) =>
          candidate.id === item.id ? { ...candidate, read: true } : candidate,
        ),
        counts: { ...prev.counts, unread: Math.max(0, prev.counts.unread - 1) },
      });
    }
    const path =
      item.source === 'notification'
        ? `/v1/inbox/notifications/${encodeURIComponent(item.id)}/read`
        : `/v1/inbox/broadcasts/${encodeURIComponent(item.id)}/read`;
    try {
      await this.http('POST', path);
      this.clearError();
    } catch (cause) {
      const rollback = changed ? { items: prev.items, counts: prev.counts } : {};
      this.store.patch({ ...rollback, error: asChimelyError(cause) });
      this.reconcileAfterFailedMutation();
    }
  }

  /** Flip an item back to unread. The override survives mark-all-read. */
  async markUnread(item: { id: InboxItemId; source: InboxItemSource }): Promise<void> {
    const prev = this.store.getSnapshot();
    const target = prev.items.find((candidate) => candidate.id === item.id);
    const changed = target?.read === true;
    if (changed) {
      this.store.patch({
        items: prev.items.map((candidate) =>
          candidate.id === item.id ? { ...candidate, read: false } : candidate,
        ),
        counts: { ...prev.counts, unread: prev.counts.unread + 1 },
      });
    }
    const path =
      item.source === 'notification'
        ? `/v1/inbox/notifications/${encodeURIComponent(item.id)}/unread`
        : `/v1/inbox/broadcasts/${encodeURIComponent(item.id)}/unread`;
    try {
      await this.http('POST', path);
      this.clearError();
    } catch (cause) {
      const rollback = changed ? { items: prev.items, counts: prev.counts } : {};
      this.store.patch({ ...rollback, error: asChimelyError(cause) });
      this.reconcileAfterFailedMutation();
    }
  }

  /**
   * Archive an item. It leaves the current view optimistically (except the
   * archived view, which cannot contain it) and an unread item leaves the
   * count. Read state is untouched.
   */
  async archive(item: { id: InboxItemId; source: InboxItemSource }): Promise<void> {
    const prev = this.store.getSnapshot();
    const target = prev.items.find((candidate) => candidate.id === item.id);
    const changed = target !== undefined && (prev.filter ?? 'default') !== 'archived';
    if (changed) {
      this.store.patch({
        items: prev.items.filter((candidate) => candidate.id !== item.id),
        counts: {
          ...prev.counts,
          unread: target.read ? prev.counts.unread : Math.max(0, prev.counts.unread - 1),
        },
      });
    }
    const path =
      item.source === 'notification'
        ? `/v1/inbox/notifications/${encodeURIComponent(item.id)}/archive`
        : `/v1/inbox/broadcasts/${encodeURIComponent(item.id)}/archive`;
    try {
      await this.http('POST', path);
      this.clearError();
    } catch (cause) {
      const rollback = changed ? { items: prev.items, counts: prev.counts } : {};
      this.store.patch({ ...rollback, error: asChimelyError(cause) });
      this.reconcileAfterFailedMutation();
    }
  }

  /** Return an item to the inbox. The override survives archive-all. */
  async unarchive(item: { id: InboxItemId; source: InboxItemSource }): Promise<void> {
    const prev = this.store.getSnapshot();
    const target = prev.items.find((candidate) => candidate.id === item.id);
    const changed = target !== undefined && (prev.filter ?? 'default') === 'archived';
    if (changed) {
      this.store.patch({
        items: prev.items.filter((candidate) => candidate.id !== item.id),
        counts: {
          ...prev.counts,
          unread: target.read ? prev.counts.unread : prev.counts.unread + 1,
        },
      });
    }
    const path =
      item.source === 'notification'
        ? `/v1/inbox/notifications/${encodeURIComponent(item.id)}/unarchive`
        : `/v1/inbox/broadcasts/${encodeURIComponent(item.id)}/unarchive`;
    try {
      await this.http('POST', path);
      this.clearError();
    } catch (cause) {
      const rollback = changed ? { items: prev.items, counts: prev.counts } : {};
      this.store.patch({ ...rollback, error: asChimelyError(cause) });
      this.reconcileAfterFailedMutation();
    }
  }

  /** Watermark move server-side. Optimistically archives everything locally. */
  async archiveAll(): Promise<void> {
    const prev = this.store.getSnapshot();
    this.store.patch({
      items: (prev.filter ?? 'default') === 'archived' ? prev.items : [],
      counts: { ...prev.counts, unread: 0 },
    });
    try {
      const response = await this.http('POST', '/v1/inbox/archive-all');
      const counts = (await response.json()) as WireCounts;
      // Only the owned field is applied. Archive state never changes unseen
      // server side, and the response was computed before a concurrent
      // markAllSeen may have landed, so its unseen value would clobber that
      // mutation's optimistic zero.
      this.store.patch({
        counts: { ...this.store.getSnapshot().counts, unread: counts.unread },
        error: null,
      });
    } catch (cause) {
      this.store.patch({ items: prev.items, counts: prev.counts, error: asChimelyError(cause) });
      this.reconcileAfterFailedMutation();
    }
  }

  /**
   * Archive every currently read item. Runs asynchronously server-side; the
   * snapshot converges via the completion hint, so there is no optimistic
   * patch.
   */
  async archiveRead(): Promise<void> {
    try {
      await this.http('POST', '/v1/inbox/archive-read');
      this.clearError();
    } catch (cause) {
      this.store.patch({ error: asChimelyError(cause) });
    }
  }

  /** Watermark move server-side. Optimistically reads everything locally. */
  async markAllRead(): Promise<void> {
    const prev = this.store.getSnapshot();
    this.store.patch({
      items: prev.items.map((item) => (item.read ? item : { ...item, read: true })),
      counts: { ...prev.counts, unread: 0 },
    });
    try {
      const response = await this.http('POST', '/v1/inbox/read-all');
      const counts = (await response.json()) as WireCounts;
      // Only the owned field is applied. The response was computed before a
      // concurrent markAllSeen may have landed, so its unseen value would
      // clobber that mutation's optimistic zero.
      this.store.patch({
        counts: { ...this.store.getSnapshot().counts, unread: counts.unread },
        error: null,
      });
    } catch (cause) {
      this.store.patch({ items: prev.items, counts: prev.counts, error: asChimelyError(cause) });
      this.reconcileAfterFailedMutation();
    }
  }

  /** The bell-open gesture. Zeroes `unseen` without touching read state. */
  async markAllSeen(): Promise<void> {
    const prev = this.store.getSnapshot();
    if (prev.counts.unseen !== 0) {
      this.store.patch({ counts: { ...prev.counts, unseen: 0 } });
    }
    try {
      const response = await this.http('POST', '/v1/inbox/seen-all');
      const counts = (await response.json()) as WireCounts;
      // Field-scoped for the same reason as markAllRead: a stale unread
      // here must not clobber a concurrent markAllRead.
      this.store.patch({
        counts: { ...this.store.getSnapshot().counts, unseen: counts.unseen },
        error: null,
      });
    } catch (cause) {
      this.store.patch({ counts: prev.counts, error: asChimelyError(cause) });
      this.reconcileAfterFailedMutation();
    }
  }

  /** Explicit rows only. A category absent here is enabled. */
  async getPreferences(): Promise<Preference[]> {
    const response = await this.http('GET', '/v1/inbox/preferences');
    const body = (await response.json()) as WirePreferenceList;
    return body.preferences;
  }

  /**
   * Partial upsert. Returns the resulting explicit rows. Category mutes are
   * applied server-side, so a successful write triggers a conditional
   * refresh of the list.
   */
  async setPreferences(preferences: Preference[]): Promise<Preference[]> {
    try {
      const response = await this.http('PUT', '/v1/inbox/preferences', {
        body: { preferences },
      });
      const body = (await response.json()) as WirePreferenceList;
      this.clearError();
      void this.refresh();
      return body.preferences;
    } catch (cause) {
      const error = asChimelyError(cause);
      this.store.patch({ error });
      throw error;
    }
  }

  // ----------------------------------------------------------- internals ---

  private clearError(): void {
    if (this.store.getSnapshot().error !== null) {
      this.store.patch({ error: null });
    }
  }

  /**
   * A failed optimistic mutation rolls back to a snapshot captured before
   * the call, which can clobber a refresh that completed in between. The
   * stored validator would then 304 the stale list back indefinitely.
   * Dropping it and refetching makes the server authoritative again.
   */
  private reconcileAfterFailedMutation(): void {
    this.etag = null;
    void this.refresh();
  }

  private async doRefresh(): Promise<void> {
    this.store.patch({ isLoading: true });
    try {
      const listPath = `/v1/inbox/items?limit=${this.pageSize}${this.filterParam()}`;
      const [pageResponse, countsResponse] = await Promise.all([
        this.http('GET', listPath, { ifNoneMatch: this.etag }),
        this.http('GET', '/v1/inbox/counts'),
      ]);
      const patch: Partial<InboxSnapshot<TPayload>> = {
        counts: (await countsResponse.json()) as WireCounts,
        isLoading: false,
        error: null,
      };
      if (pageResponse.status === 200) {
        this.etag = pageResponse.headers.get('ETag');
        const page = (await pageResponse.json()) as WireInboxPage;
        Object.assign(patch, this.mergeFirstPage(page));
      }
      this.store.patch(patch);
    } catch (cause) {
      this.store.patch({ isLoading: false, error: asChimelyError(cause) });
    }
  }

  /**
   * Folds a fresh first page into the loaded list. Items older than the
   * page boundary were fetched via fetchMore and are kept, so a refresh
   * never collapses the user's scroll position. The kept tail is provably
   * contiguous with the page only when the page shares at least one item
   * with the loaded list. A full page with zero overlap means more than a
   * page of changes arrived while disconnected. Keeping the tail then
   * would leave a hole that never heals because the stored ETag is
   * current, so the list resets to the page instead.
   */
  private mergeFirstPage(page: WireInboxPage): Partial<InboxSnapshot<TPayload>> {
    const pageItems = page.items.map((item) => toItem<TPayload>(item));
    const existing = this.store.getSnapshot();
    const existingIds = new Set(existing.items.map((item) => item.id));
    // Fresh array per merge so consumers can detect arrivals by identity.
    const lastRefreshNewItemIds = pageItems
      .filter((item) => !existingIds.has(item.id))
      .map((item) => item.id);
    const boundary = pageItems[pageItems.length - 1];
    if (page.next_cursor === null || boundary === undefined) {
      // A cursor-less page with no next page is the entire inbox.
      this.cursor = null;
      return { items: pageItems, hasMore: false, lastRefreshNewItemIds };
    }
    const pageIds = new Set(pageItems.map((item) => item.id));
    const tail = existing.items.filter(
      (item) => !pageIds.has(item.id) && isOlderThan(item, boundary),
    );
    const contiguous = existing.items.some((item) => pageIds.has(item.id));
    if (tail.length === 0 || !contiguous) {
      this.cursor = page.next_cursor;
      return { items: pageItems, hasMore: true, lastRefreshNewItemIds };
    }
    return {
      items: [...pageItems, ...tail],
      hasMore: existing.hasMore,
      lastRefreshNewItemIds,
    };
  }

  private async doFetchMore(limit?: number): Promise<void> {
    try {
      const params = new URLSearchParams({
        limit: String(Math.min(100, Math.max(1, limit ?? this.pageSize))),
      });
      const filter = this.store.getSnapshot().filter ?? 'default';
      if (filter !== 'default') {
        params.set('filter', filter);
      }
      if (this.cursor !== null) {
        params.set('cursor', this.cursor);
      }
      const response = await this.http('GET', `/v1/inbox/items?${params.toString()}`);
      const page = (await response.json()) as WireInboxPage;
      const snapshot = this.store.getSnapshot();
      const loaded = new Set(snapshot.items.map((item) => item.id));
      const appended = page.items
        .filter((item) => !loaded.has(item.id as InboxItemId))
        .map((item) => toItem<TPayload>(item));
      this.cursor = page.next_cursor;
      this.store.patch({
        items: [...snapshot.items, ...appended],
        hasMore: page.next_cursor !== null,
        error: null,
      });
    } catch (cause) {
      this.store.patch({ error: asChimelyError(cause) });
    }
  }

  private async http(
    method: string,
    path: string,
    init?: { ifNoneMatch?: string | null; body?: unknown },
  ): Promise<Response> {
    const headers: Record<string, string> = {
      'X-Chimely-Environment': this.config.environment,
      'X-Chimely-Subscriber': this.config.subscriberId,
    };
    if (this.config.subscriberHash !== undefined) {
      headers['X-Chimely-Subscriber-Hash'] = this.config.subscriberHash;
    }
    if (init?.ifNoneMatch) {
      headers['If-None-Match'] = init.ifNoneMatch;
    }
    let body: string | undefined;
    if (init?.body !== undefined) {
      headers['Content-Type'] = 'application/json';
      body = JSON.stringify(init.body);
    }
    let response: Response;
    try {
      response = await this.fetchFn(this.baseUrl + path, { method, headers, body });
    } catch (cause) {
      throw networkError(cause);
    }
    if (!response.ok && response.status !== 304) {
      throw await errorFromResponse(response);
    }
    return response;
  }

  // ----------------------------------------------------------------- sse ---

  /**
   * Auth rides query parameters (EventSource cannot set headers). The
   * reconnect loop recreates the source, which loses the platform's
   * automatic Last-Event-ID header, so the resume token rides the URL too.
   */
  private streamUrl(): string {
    const params = new URLSearchParams({
      environment: this.config.environment,
      subscriber_id: this.config.subscriberId,
    });
    if (this.config.subscriberHash !== undefined) {
      params.set('subscriber_hash', this.config.subscriberHash);
    }
    if (this.lastEventId !== null) {
      params.set('last_event_id', this.lastEventId);
    }
    return `${this.baseUrl}/v1/inbox/stream?${params.toString()}`;
  }

  private openStream(): void {
    const generation = this.generation;
    let source: EventSourceLike;
    try {
      source = this.createSource(this.streamUrl());
    } catch {
      this.handleStreamFailure();
      return;
    }
    this.source = source;
    source.addEventListener('open', () => {
      if (generation !== this.generation) {
        return;
      }
      this.attempts = 0;
      this.retryOverrideMs = null;
      this.store.patch({ status: 'connected' });
      void this.refresh();
    });
    source.addEventListener('hint', (event) => {
      if (generation !== this.generation) {
        return;
      }
      if (event.lastEventId) {
        this.lastEventId = event.lastEventId;
      }
      void this.refresh();
    });
    source.addEventListener('retry', (event) => {
      if (generation !== this.generation) {
        return;
      }
      const ms = Number(event.data);
      if (Number.isFinite(ms) && ms >= 0) {
        this.retryOverrideMs = ms;
      }
    });
    source.addEventListener('error', (event) => {
      if (generation !== this.generation) {
        return;
      }
      if (event.lastEventId) {
        this.lastEventId = event.lastEventId;
      }
      this.handleStreamFailure();
    });
  }

  private handleStreamFailure(): void {
    this.source?.close();
    this.source = null;
    this.attempts += 1;
    if (this.attempts >= this.backoff.maxAttempts) {
      this.generation += 1;
      this.store.patch({
        status: 'closed',
        error: new ChimelyError('stream closed after maxAttempts consecutive failures', {
          code: 'connection_failed',
        }),
      });
      return;
    }
    const delay = this.retryOverrideMs ?? backoffDelayMs(this.attempts, this.backoff);
    this.retryOverrideMs = null;
    this.store.patch({ status: 'reconnecting' });
    const generation = this.generation;
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      if (generation !== this.generation) {
        return;
      }
      this.openStream();
    }, delay);
  }
}
