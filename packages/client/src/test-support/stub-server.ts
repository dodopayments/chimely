/**
 * In-process stub of the Chimely subscriber-plane API, typed against the
 * generated wire types in ../generated/api. Never touches server/. The
 * generated types are the contract.
 */

import type { components } from '../generated/api';
import type { EventSourceLike } from '../types';

type WireItem = components['schemas']['InboxItem'];
type WirePage = components['schemas']['InboxPage'];
type WireCounts = components['schemas']['InboxCounts'];
type WirePreference = components['schemas']['Preference'];
type WireError = components['schemas']['Error'];
type WireErrorCode = WireError['error']['code'];

export interface RecordedRequest {
  method: string;
  path: string;
  search: URLSearchParams;
  headers: Record<string, string>;
  body: unknown;
  status: number;
}

export interface StubServerOptions {
  environment?: string;
  subscriberId?: string;
  /** When set, X-Chimely-Subscriber-Hash must equal it or the stub returns 401. */
  requireHash?: string;
  baseTimeMs?: number;
}

interface InjectedFailure {
  method: string;
  pathIncludes: string;
  status: number;
  code: WireErrorCode;
  message: string;
}

type StreamListener = (event: { data?: string; lastEventId?: string }) => void;

export class FakeEventSource implements EventSourceLike {
  readonly url: string;
  closed = false;
  private readonly listeners = new Map<string, Set<StreamListener>>();

  constructor(url: string) {
    this.url = url;
  }

  addEventListener(type: string, listener: StreamListener): void {
    let set = this.listeners.get(type);
    if (!set) {
      set = new Set();
      this.listeners.set(type, set);
    }
    set.add(listener);
  }

  close(): void {
    this.closed = true;
  }

  emit(type: string, event: { data?: string; lastEventId?: string } = {}): void {
    for (const listener of this.listeners.get(type) ?? []) {
      listener(event);
    }
  }
}

// Cursor format is opaque to the client.
function encodeCursor(item: WireItem): string {
  return JSON.stringify([item.occurred_at, item.id]);
}

function decodeCursor(cursor: string): [string, string] {
  return JSON.parse(cursor) as [string, string];
}

function errorBody(code: WireErrorCode, message: string): WireError {
  return { error: { code, message } };
}

function json(status: number, body: unknown, headers?: Record<string, string>): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json', ...headers },
  });
}

export class StubServer {
  readonly environment: string;
  readonly subscriberId: string;
  readonly requireHash: string | undefined;
  readonly requests: RecordedRequest[] = [];
  readonly sources: FakeEventSource[] = [];

  private items: WireItem[] = [];
  private prefs: WirePreference[] = [];
  private unseen = 0;
  private version = 1;
  private seq = 0;
  private readonly baseTimeMs: number;
  private readonly failures: InjectedFailure[] = [];

  constructor(options: StubServerOptions = {}) {
    this.environment = options.environment ?? 'env-test';
    this.subscriberId = options.subscriberId ?? 'usr_1';
    this.requireHash = options.requireHash;
    this.baseTimeMs = options.baseTimeMs ?? Date.UTC(2026, 0, 1);
  }

  // ------------------------------------------------------- state control ---

  /** Inserts a direct notification. Items stay sorted newest-first. */
  addNotification(
    overrides: Partial<
      Pick<WireItem, 'category' | 'payload' | 'read' | 'archived' | 'occurred_at'>
    > & {
      seen?: boolean;
    } = {},
  ): WireItem {
    return this.addItem('notification', overrides);
  }

  /** Inserts a broadcast. Items stay sorted newest-first. */
  addBroadcast(
    overrides: Partial<
      Pick<WireItem, 'category' | 'payload' | 'read' | 'archived' | 'occurred_at'>
    > & {
      seen?: boolean;
    } = {},
  ): WireItem {
    return this.addItem('broadcast', overrides);
  }

  private addItem(
    source: WireItem['source'],
    overrides: Partial<
      Pick<WireItem, 'category' | 'payload' | 'read' | 'archived' | 'occurred_at'>
    > & {
      seen?: boolean;
    },
  ): WireItem {
    this.seq += 1;
    const prefix = source === 'notification' ? 'notif' : 'bcast';
    const item: WireItem = {
      id: `${prefix}_${String(this.seq).padStart(26, '0')}`,
      source,
      category: overrides.category ?? 'test.event',
      payload: overrides.payload ?? { title: `item ${this.seq}` },
      occurred_at:
        overrides.occurred_at ?? new Date(this.baseTimeMs + this.seq * 1000).toISOString(),
      read: overrides.read ?? false,
      archived: overrides.archived ?? false,
    };
    this.items.push(item);
    this.items.sort((a, b) => {
      if (a.occurred_at !== b.occurred_at) {
        return a.occurred_at < b.occurred_at ? 1 : -1;
      }
      return a.id < b.id ? 1 : a.id > b.id ? -1 : 0;
    });
    if (!overrides.seen) {
      this.unseen += 1;
    }
    this.version += 1;
    return item;
  }

  counts(): WireCounts {
    return {
      unread: this.items.filter((item) => !item.read && !item.archived).length,
      unseen: this.unseen,
    };
  }

  preferences(): WirePreference[] {
    return [...this.prefs];
  }

  setPreferenceRow(pref: WirePreference): void {
    this.prefs = this.prefs.filter(
      (row) => !(row.category === pref.category && row.channel === pref.channel),
    );
    if (!pref.enabled) {
      this.prefs.push(pref);
    }
    this.version += 1;
  }

  /** The next matching request fails with this status and error code. */
  failNext(
    method: string,
    pathIncludes: string,
    failure: { status: number; code: WireErrorCode; message?: string },
  ): void {
    this.failures.push({
      method,
      pathIncludes,
      status: failure.status,
      code: failure.code,
      message: failure.message ?? 'injected failure',
    });
  }

  requestsFor(pathIncludes: string): RecordedRequest[] {
    return this.requests.filter((request) => request.path.includes(pathIncludes));
  }

  // ------------------------------------------------------------- streams ---

  readonly createEventSource = (url: string): EventSourceLike => {
    const source = new FakeEventSource(url);
    this.sources.push(source);
    return source;
  };

  /** The most recently created stream. */
  stream(): FakeEventSource {
    const source = this.sources[this.sources.length - 1];
    if (!source) {
      throw new Error('no EventSource has been created yet');
    }
    return source;
  }

  openStream(): void {
    this.stream().emit('open');
  }

  emitHint(lastEventId?: string): void {
    this.stream().emit('hint', {
      data: JSON.stringify({ reason: 'test' }),
      ...(lastEventId === undefined ? {} : { lastEventId }),
    });
  }

  /**
   * Mimics the graceful-close frame the server emits on shutdown. A named
   * retry event whose data is the next delay in milliseconds.
   */
  emitRetry(ms: number): void {
    this.stream().emit('retry', { data: String(ms) });
  }

  dropStream(): void {
    this.stream().emit('error');
  }

  // --------------------------------------------------------------- fetch ---

  readonly fetchFn: typeof fetch = async (input, init) => {
    const url = new URL(
      typeof input === 'string' ? input : input instanceof URL ? input.href : input.url,
    );
    const method = (init?.method ?? 'GET').toUpperCase();
    const headers: Record<string, string> = {};
    new Headers(init?.headers).forEach((value, key) => {
      headers[key.toLowerCase()] = value;
    });
    const body = typeof init?.body === 'string' ? JSON.parse(init.body) : undefined;
    const record: RecordedRequest = {
      method,
      path: url.pathname,
      search: url.searchParams,
      headers,
      body,
      status: 0,
    };
    this.requests.push(record);
    const response = this.route(method, url, headers, body);
    record.status = response.status;
    return response;
  };

  private route(
    method: string,
    url: URL,
    headers: Record<string, string>,
    body: unknown,
  ): Response {
    const path = url.pathname;
    const failureIndex = this.failures.findIndex(
      (failure) => failure.method === method && path.includes(failure.pathIncludes),
    );
    if (failureIndex !== -1) {
      const [failure] = this.failures.splice(failureIndex, 1);
      if (failure) {
        return json(failure.status, errorBody(failure.code, failure.message));
      }
    }

    if (
      headers['x-chimely-environment'] !== this.environment ||
      headers['x-chimely-subscriber'] !== this.subscriberId ||
      (this.requireHash !== undefined && headers['x-chimely-subscriber-hash'] !== this.requireHash)
    ) {
      return json(401, errorBody('unauthorized', 'missing or invalid subscriber auth'));
    }

    if (method === 'GET' && path === '/v1/inbox/items') {
      return this.listItems(url, headers);
    }
    if (method === 'GET' && path === '/v1/inbox/counts') {
      return json(200, this.counts());
    }
    const readMatch = path.match(
      /^\/v1\/inbox\/(notifications|broadcasts)\/([^/]+)\/(read|unread|archive|unarchive)$/,
    );
    if (method === 'POST' && readMatch) {
      const source = readMatch[1] === 'notifications' ? 'notification' : 'broadcast';
      const item = this.items.find((row) => row.id === readMatch[2] && row.source === source);
      if (!item) {
        return json(404, errorBody('not_found', 'no such item in this environment'));
      }
      const action = readMatch[3];
      if (action === 'read' || action === 'unread') {
        const wantRead = action === 'read';
        if (item.read !== wantRead) {
          item.read = wantRead;
          this.version += 1;
        }
      } else {
        const wantArchived = action === 'archive';
        if (item.archived !== wantArchived) {
          item.archived = wantArchived;
          this.version += 1;
        }
      }
      return new Response(null, { status: 204 });
    }
    if (method === 'POST' && path === '/v1/inbox/archive-all') {
      for (const item of this.items) {
        item.archived = true;
      }
      this.version += 1;
      return json(200, this.counts());
    }
    if (method === 'POST' && path === '/v1/inbox/archive-read') {
      // The real server runs this as an async chunked job; the stub applies
      // it synchronously and the client converges on the next refresh.
      for (const item of this.items) {
        if (item.read) {
          item.archived = true;
        }
      }
      this.version += 1;
      return new Response(null, { status: 202 });
    }
    if (method === 'POST' && path === '/v1/inbox/read-all') {
      for (const item of this.items) {
        item.read = true;
      }
      this.version += 1;
      return json(200, this.counts());
    }
    if (method === 'POST' && path === '/v1/inbox/seen-all') {
      this.unseen = 0;
      this.version += 1;
      return json(200, this.counts());
    }
    if (method === 'GET' && path === '/v1/inbox/preferences') {
      return json(200, { preferences: this.preferences() });
    }
    if (method === 'PUT' && path === '/v1/inbox/preferences') {
      const writes = (body as { preferences: WirePreference[] }).preferences;
      for (const pref of writes) {
        this.setPreferenceRow(pref);
      }
      return json(200, { preferences: this.preferences() });
    }
    return json(404, errorBody('not_found', `no route for ${method} ${path}`));
  }

  private listItems(url: URL, headers: Record<string, string>): Response {
    const cursor = url.searchParams.get('cursor');
    const limit = Number(url.searchParams.get('limit') ?? '20');
    const filter = url.searchParams.get('filter');
    const visible =
      filter === 'unread'
        ? this.items.filter((item) => !item.read && !item.archived)
        : filter === 'archived'
          ? this.items.filter((item) => item.archived)
          : this.items.filter((item) => !item.archived);
    const etag = `"v${this.version}:${filter ?? ''}:${cursor ?? ''}"`;
    if (headers['if-none-match'] === etag) {
      return new Response(null, { status: 304 });
    }
    let start = 0;
    if (cursor !== null) {
      const [occurredAt, id] = decodeCursor(cursor);
      start = visible.findIndex(
        (item) =>
          item.occurred_at < occurredAt || (item.occurred_at === occurredAt && item.id < id),
      );
      if (start === -1) {
        start = visible.length;
      }
    }
    const pageItems = visible.slice(start, start + limit);
    const last = pageItems[pageItems.length - 1];
    const hasMore = start + limit < visible.length;
    const page: WirePage = {
      items: pageItems,
      next_cursor: hasMore && last ? encodeCursor(last) : null,
    };
    return json(200, page, { ETag: etag });
  }
}

export function createStubServer(options: StubServerOptions = {}): StubServer {
  return new StubServer(options);
}
