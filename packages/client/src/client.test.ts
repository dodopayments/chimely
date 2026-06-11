import { afterEach, describe, expect, test, vi } from 'vitest';
import { DronteClient } from './client';
import { DronteError } from './errors';
import type { StubServer } from './test-support/stub-server';
import { createStubServer } from './test-support/stub-server';
import type { DronteClientConfig, InboxItem, InboxSnapshot } from './types';

function makeClient(stub: StubServer, config: Partial<DronteClientConfig> = {}): DronteClient {
  return new DronteClient({
    serverUrl: 'https://dronte.test',
    environment: stub.environment,
    subscriberId: stub.subscriberId,
    fetchFn: stub.fetchFn,
    createEventSource: stub.createEventSource,
    ...config,
  });
}

function must<T>(value: T | undefined, label = 'value'): T {
  if (value === undefined) {
    throw new Error(`expected ${label} to be present`);
  }
  return value;
}

/** Connects, opens the stream, and waits for the initial load to settle. */
async function connectAndLoad(client: DronteClient, stub: StubServer): Promise<void> {
  client.connect();
  stub.openStream();
  await vi.waitFor(() => {
    expect(client.getSnapshot().isLoading).toBe(false);
    expect(stub.requestsFor('/v1/inbox/counts').length).toBeGreaterThan(0);
  });
  await client.refresh();
}

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe('connection and auth', () => {
  test('connect opens the stream with auth query params and loads page one + counts', async () => {
    const stub = createStubServer({ requireHash: 'deadbeef' });
    stub.addNotification({ payload: { title: 'hello' } });
    const client = makeClient(stub, { subscriberHash: 'deadbeef' });

    expect(client.getSnapshot().status).toBe('idle');
    client.connect();
    expect(client.getSnapshot().status).toBe('connecting');

    const streamUrl = new URL(stub.stream().url);
    expect(streamUrl.pathname).toBe('/v1/inbox/stream');
    expect(streamUrl.searchParams.get('environment')).toBe(stub.environment);
    expect(streamUrl.searchParams.get('subscriber_id')).toBe(stub.subscriberId);
    expect(streamUrl.searchParams.get('subscriber_hash')).toBe('deadbeef');

    stub.openStream();
    expect(client.getSnapshot().status).toBe('connected');

    await vi.waitFor(() => {
      expect(client.getSnapshot().items).toHaveLength(1);
    });
    expect(client.getSnapshot().counts).toEqual({ unread: 1, unseen: 1 });

    const listRequest = must(stub.requestsFor('/v1/inbox/items')[0]);
    expect(listRequest.headers['x-dronte-environment']).toBe(stub.environment);
    expect(listRequest.headers['x-dronte-subscriber']).toBe(stub.subscriberId);
    expect(listRequest.headers['x-dronte-subscriber-hash']).toBe('deadbeef');
  });

  test('subscriber hash is omitted everywhere when not configured', async () => {
    const stub = createStubServer();
    const client = makeClient(stub);
    await connectAndLoad(client, stub);

    expect(new URL(stub.stream().url).searchParams.has('subscriber_hash')).toBe(false);
    const listRequest = must(stub.requestsFor('/v1/inbox/items')[0]);
    expect('x-dronte-subscriber-hash' in listRequest.headers).toBe(false);
  });

  test('a wrong hash surfaces unauthorized on the error channel', async () => {
    const stub = createStubServer({ requireHash: 'right' });
    const client = makeClient(stub, { subscriberHash: 'wrong' });
    client.connect();
    await vi.waitFor(() => {
      expect(client.getSnapshot().error).toBeInstanceOf(DronteError);
    });
    expect(client.getSnapshot().error?.code).toBe('unauthorized');
    expect(client.getSnapshot().error?.status).toBe(401);
  });

  test('connect is idempotent and close stops the stream but keeps the store readable', async () => {
    const stub = createStubServer();
    stub.addNotification();
    const client = makeClient(stub);
    await connectAndLoad(client, stub);

    client.connect();
    expect(stub.sources).toHaveLength(1);

    client.close();
    expect(client.getSnapshot().status).toBe('closed');
    expect(stub.stream().closed).toBe(true);
    expect(client.getSnapshot().items).toHaveLength(1);

    client.connect();
    expect(stub.sources).toHaveLength(2);
  });
});

describe('conditional refetch (ETag)', () => {
  test('a hint with unchanged state is a 304 and the list keeps identity', async () => {
    const stub = createStubServer();
    stub.addNotification();
    const client = makeClient(stub);
    await connectAndLoad(client, stub);

    const itemsBefore = client.getSnapshot().items;
    stub.emitHint();
    await vi.waitFor(() => {
      expect(stub.requestsFor('/v1/inbox/items').length).toBeGreaterThanOrEqual(2);
      expect(client.getSnapshot().isLoading).toBe(false);
    });

    const conditional = must(stub.requestsFor('/v1/inbox/items').at(-1));
    expect(conditional.headers['if-none-match']).toBeDefined();
    expect(conditional.status).toBe(304);
    expect(client.getSnapshot().items).toBe(itemsBefore);
  });

  test('a hint after a change refetches with 200 and updates items + counts', async () => {
    const stub = createStubServer();
    stub.addNotification({ payload: { title: 'first' } });
    const client = makeClient(stub);
    await connectAndLoad(client, stub);
    expect(client.getSnapshot().items).toHaveLength(1);

    stub.addNotification({ payload: { title: 'second' } });
    stub.emitHint();
    await vi.waitFor(() => {
      expect(client.getSnapshot().items).toHaveLength(2);
    });
    const newest = must(client.getSnapshot().items[0]);
    expect(newest.payload).toEqual({ title: 'second' });
    expect(client.getSnapshot().counts.unread).toBe(2);
    expect(must(stub.requestsFor('/v1/inbox/items').at(-1)).status).toBe(200);
  });

  test('reconnect refetches conditionally (mostly 304s after a deploy-style drop)', async () => {
    vi.useFakeTimers();
    vi.spyOn(Math, 'random').mockReturnValue(0.5);
    const stub = createStubServer();
    stub.addNotification();
    const client = makeClient(stub);
    client.connect();
    stub.openStream();
    await vi.advanceTimersByTimeAsync(0);

    const before = stub.requestsFor('/v1/inbox/items').length;
    stub.dropStream();
    await vi.advanceTimersByTimeAsync(1000);
    stub.openStream();
    await vi.advanceTimersByTimeAsync(0);

    const after = stub.requestsFor('/v1/inbox/items');
    expect(after.length).toBe(before + 1);
    expect(must(after.at(-1)).status).toBe(304);
    expect(client.getSnapshot().status).toBe('connected');
  });
});

describe('reconnect loop', () => {
  test('a drop schedules a jittered exponential reconnect and resumes with the last event id', async () => {
    vi.useFakeTimers();
    vi.spyOn(Math, 'random').mockReturnValue(0.5);
    const stub = createStubServer();
    const client = makeClient(stub);
    client.connect();
    stub.openStream();
    await vi.advanceTimersByTimeAsync(0);

    stub.emitHint('resume-token-1');
    await vi.advanceTimersByTimeAsync(0);

    stub.dropStream();
    expect(client.getSnapshot().status).toBe('reconnecting');
    expect(stub.stream().closed).toBe(true);

    await vi.advanceTimersByTimeAsync(999);
    expect(stub.sources).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(stub.sources).toHaveLength(2);
    expect(new URL(stub.stream().url).searchParams.get('last_event_id')).toBe('resume-token-1');
  });

  test('consecutive failures back off exponentially up to the cap and reset on open', async () => {
    vi.useFakeTimers();
    vi.spyOn(Math, 'random').mockReturnValue(0.5);
    const stub = createStubServer();
    const client = makeClient(stub, {
      backoff: { initialDelayMs: 100, maxDelayMs: 400, multiplier: 2, jitter: 0 },
    });
    client.connect();

    // Failures without an intervening open: 100, 200, 400, 400 (capped).
    for (const expected of [100, 200, 400, 400]) {
      const count = stub.sources.length;
      stub.dropStream();
      await vi.advanceTimersByTimeAsync(expected - 1);
      expect(stub.sources).toHaveLength(count);
      await vi.advanceTimersByTimeAsync(1);
      expect(stub.sources).toHaveLength(count + 1);
    }

    // A successful open resets the consecutive failure count.
    stub.openStream();
    await vi.advanceTimersByTimeAsync(0);
    const count = stub.sources.length;
    stub.dropStream();
    await vi.advanceTimersByTimeAsync(100);
    expect(stub.sources).toHaveLength(count + 1);
  });

  test('reconnect delays are jittered within ±jitter of the base delay', async () => {
    vi.useFakeTimers();
    // The spy wraps the fake timer, so scheduled delays can be read directly
    // instead of probing the clock millisecond by millisecond.
    const setTimeoutSpy = vi.spyOn(globalThis, 'setTimeout');
    const stub = createStubServer();
    const client = makeClient(stub, {
      backoff: { initialDelayMs: 1000, multiplier: 1, jitter: 0.5 },
    });
    client.connect();

    const delays: number[] = [];
    for (let i = 0; i < 40; i += 1) {
      setTimeoutSpy.mockClear();
      stub.dropStream();
      const scheduled = setTimeoutSpy.mock.calls.at(-1)?.[1];
      if (typeof scheduled !== 'number') {
        throw new Error('no reconnect was scheduled');
      }
      delays.push(scheduled);
      await vi.runOnlyPendingTimersAsync();
    }
    for (const delay of delays) {
      expect(delay).toBeGreaterThanOrEqual(500);
      expect(delay).toBeLessThanOrEqual(1500);
    }
    expect(new Set(delays).size).toBeGreaterThan(5);
  });

  test('the server retry directive overrides exactly one reconnect delay', async () => {
    vi.useFakeTimers();
    vi.spyOn(Math, 'random').mockReturnValue(0.5);
    const stub = createStubServer();
    const client = makeClient(stub);
    client.connect();
    stub.openStream();
    await vi.advanceTimersByTimeAsync(0);

    stub.emitRetry(5000);
    stub.dropStream();
    await vi.advanceTimersByTimeAsync(4999);
    expect(stub.sources).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(stub.sources).toHaveLength(2);

    // The override is consumed. The next failure uses the backoff schedule.
    stub.dropStream();
    await vi.advanceTimersByTimeAsync(2000);
    expect(stub.sources).toHaveLength(3);
  });

  test('maxAttempts consecutive failures close the client with a connection error', async () => {
    vi.useFakeTimers();
    vi.spyOn(Math, 'random').mockReturnValue(0.5);
    const stub = createStubServer();
    const client = makeClient(stub, {
      backoff: { initialDelayMs: 10, jitter: 0, maxAttempts: 2 },
    });
    client.connect();

    stub.dropStream();
    expect(client.getSnapshot().status).toBe('reconnecting');
    await vi.advanceTimersByTimeAsync(10);
    expect(stub.sources).toHaveLength(2);

    stub.dropStream();
    expect(client.getSnapshot().status).toBe('closed');
    expect(client.getSnapshot().error?.code).toBe('connection_failed');
    await vi.advanceTimersByTimeAsync(60000);
    expect(stub.sources).toHaveLength(2);
  });
});

describe('optimistic read state', () => {
  test('markRead updates the snapshot synchronously and never flickers on success', async () => {
    const stub = createStubServer();
    stub.addNotification();
    stub.addBroadcast();
    const client = makeClient(stub);
    await connectAndLoad(client, stub);

    const target = must(client.getSnapshot().items.find((item) => item.source === 'notification'));
    const readStates: boolean[] = [];
    client.subscribe(() => {
      const current = client.getSnapshot().items.find((item) => item.id === target.id);
      if (current) {
        readStates.push(current.read);
      }
    });

    const promise = client.markRead(target);
    expect(must(client.getSnapshot().items.find((i) => i.id === target.id)).read).toBe(true);
    expect(client.getSnapshot().counts.unread).toBe(1);
    await promise;

    expect(readStates.length).toBeGreaterThan(0);
    expect(readStates.every((state) => state)).toBe(true);
    expect(client.getSnapshot().error).toBeNull();
    const request = must(stub.requestsFor(`/v1/inbox/notifications/${target.id}/read`)[0]);
    expect(request.method).toBe('POST');
    expect(request.status).toBe(204);
  });

  test('markRead routes broadcasts to the broadcast endpoint', async () => {
    const stub = createStubServer();
    const broadcast = stub.addBroadcast();
    const client = makeClient(stub);
    await connectAndLoad(client, stub);

    await client.markRead({ id: broadcast.id as InboxItem['id'], source: 'broadcast' });
    expect(stub.requestsFor(`/v1/inbox/broadcasts/${broadcast.id}/read`)).toHaveLength(1);
    expect(stub.counts().unread).toBe(0);
  });

  test('a failed markRead rolls back and surfaces the server error code', async () => {
    const stub = createStubServer();
    const item = stub.addNotification();
    const client = makeClient(stub);
    await connectAndLoad(client, stub);

    stub.failNext('POST', `/v1/inbox/notifications/${item.id}/read`, {
      status: 500,
      code: 'internal',
    });
    const before = client.getSnapshot();
    await client.markRead({ id: item.id as InboxItem['id'], source: 'notification' });

    const after = client.getSnapshot();
    expect(must(after.items[0]).read).toBe(false);
    expect(after.counts).toEqual(before.counts);
    expect(after.error).toBeInstanceOf(DronteError);
    expect(after.error?.code).toBe('internal');
    expect(after.error?.status).toBe(500);
  });

  test('markAllRead is optimistic, reconciles with server counts, and rolls back on failure', async () => {
    const stub = createStubServer();
    stub.addNotification();
    stub.addBroadcast();
    const client = makeClient(stub);
    await connectAndLoad(client, stub);

    const promise = client.markAllRead();
    expect(client.getSnapshot().items.every((item) => item.read)).toBe(true);
    expect(client.getSnapshot().counts.unread).toBe(0);
    await promise;
    expect(client.getSnapshot().counts.unread).toBe(0);
    expect(client.getSnapshot().error).toBeNull();

    stub.addNotification();
    await client.refresh();
    stub.failNext('POST', '/v1/inbox/read-all', { status: 429, code: 'rate_limited' });
    await client.markAllRead();
    expect(client.getSnapshot().counts.unread).toBe(1);
    expect(client.getSnapshot().items.some((item) => !item.read)).toBe(true);
    expect(client.getSnapshot().error?.code).toBe('rate_limited');
  });

  test('markAllSeen zeroes unseen without touching read state and rolls back on failure', async () => {
    const stub = createStubServer();
    stub.addNotification();
    const client = makeClient(stub);
    await connectAndLoad(client, stub);
    expect(client.getSnapshot().counts).toEqual({ unread: 1, unseen: 1 });

    const promise = client.markAllSeen();
    expect(client.getSnapshot().counts.unseen).toBe(0);
    await promise;
    expect(client.getSnapshot().counts).toEqual({ unread: 1, unseen: 0 });
    expect(must(client.getSnapshot().items[0]).read).toBe(false);

    stub.addNotification();
    await client.refresh();
    stub.failNext('POST', '/v1/inbox/seen-all', { status: 500, code: 'internal' });
    await client.markAllSeen();
    expect(client.getSnapshot().counts.unseen).toBe(1);
    expect(client.getSnapshot().error?.code).toBe('internal');
  });

  test('concurrent markAllRead and markAllSeen do not clobber each other', async () => {
    const stub = createStubServer();
    stub.addNotification();
    stub.addNotification();
    let releaseSeenAll = (): void => {};
    const gate = new Promise<void>((resolve) => {
      releaseSeenAll = resolve;
    });
    const client = makeClient(stub, {
      fetchFn: async (input, init) => {
        if (String(input).includes('/seen-all')) {
          await gate;
        }
        return stub.fetchFn(input, init);
      },
    });
    await connectAndLoad(client, stub);
    expect(client.getSnapshot().counts).toEqual({ unread: 2, unseen: 2 });

    // The seen-all request hangs before reaching the server, so the
    // read-all response is computed while unseen is still 2 server-side.
    const seenPromise = client.markAllSeen();
    expect(client.getSnapshot().counts.unseen).toBe(0);
    await client.markAllRead();

    // The stale unseen in the read-all response must not undo the
    // optimistic zero of the still-in-flight markAllSeen.
    expect(client.getSnapshot().counts).toEqual({ unread: 0, unseen: 0 });

    releaseSeenAll();
    await seenPromise;
    expect(client.getSnapshot().counts).toEqual({ unread: 0, unseen: 0 });
    expect(client.getSnapshot().error).toBeNull();
  });

  test('a failed mutation that raced a refresh reconciles instead of resurrecting the stale list', async () => {
    const stub = createStubServer();
    const first = stub.addNotification();
    let releasePost = (): void => {};
    const gate = new Promise<void>((resolve) => {
      releasePost = resolve;
    });
    const client = makeClient(stub, {
      fetchFn: async (input, init) => {
        if ((init?.method ?? 'GET') === 'POST') {
          await gate;
        }
        return stub.fetchFn(input, init);
      },
    });
    await connectAndLoad(client, stub);

    stub.failNext('POST', `/v1/inbox/notifications/${first.id}/read`, {
      status: 500,
      code: 'internal',
    });
    const markPromise = client.markRead({
      id: first.id as InboxItem['id'],
      source: 'notification',
    });

    // A refresh completes while the doomed POST is in flight.
    stub.addNotification({ payload: { title: 'arrived mid-mutation' } });
    await client.refresh();
    expect(client.getSnapshot().items).toHaveLength(2);

    releasePost();
    await markPromise;

    // The rollback restored the pre-mutation snapshot. Reconciliation must
    // refetch past the stored ETag so the refreshed item is not lost.
    await vi.waitFor(() => {
      expect(client.getSnapshot().items).toHaveLength(2);
      expect(client.getSnapshot().counts.unread).toBe(2);
    });
  });

  test('network failures surface as code network', async () => {
    const stub = createStubServer();
    const item = stub.addNotification();
    const client = makeClient(stub);
    await connectAndLoad(client, stub);

    const failing = makeClient(stub, {
      fetchFn: () => Promise.reject(new TypeError('fetch failed')),
    });
    // Reuse the loaded snapshot shape by acting on the fresh client directly.
    await failing.markRead({ id: item.id as InboxItem['id'], source: 'notification' });
    expect(failing.getSnapshot().error?.code).toBe('network');
  });
});

describe('merged-stream keyset pagination', () => {
  test('pages through both sources in (occurred_at, id) descending order', async () => {
    const stub = createStubServer();
    for (let i = 0; i < 9; i += 1) {
      if (i % 2 === 0) {
        stub.addNotification();
      } else {
        stub.addBroadcast();
      }
    }
    const client = makeClient(stub, { pageSize: 4 });
    await connectAndLoad(client, stub);

    let snapshot = client.getSnapshot();
    expect(snapshot.items).toHaveLength(4);
    expect(snapshot.hasMore).toBe(true);

    await client.fetchMore();
    await client.fetchMore();
    snapshot = client.getSnapshot();
    expect(snapshot.items).toHaveLength(9);
    expect(snapshot.hasMore).toBe(false);

    const occurredAts = snapshot.items.map((item) => item.occurredAt);
    expect([...occurredAts].sort().reverse()).toEqual(occurredAts);
    expect(snapshot.items.some((item) => item.source === 'notification')).toBe(true);
    expect(snapshot.items.some((item) => item.source === 'broadcast')).toBe(true);
    expect(new Set(snapshot.items.map((item) => item.id)).size).toBe(9);

    const requestCount = stub.requestsFor('/v1/inbox/items').length;
    await client.fetchMore();
    expect(stub.requestsFor('/v1/inbox/items')).toHaveLength(requestCount);
  });

  test('concurrent fetchMore calls collapse into one request', async () => {
    const stub = createStubServer();
    for (let i = 0; i < 6; i += 1) {
      stub.addNotification();
    }
    const client = makeClient(stub, { pageSize: 2 });
    await connectAndLoad(client, stub);

    const before = stub.requestsFor('/v1/inbox/items').length;
    await Promise.all([client.fetchMore(), client.fetchMore()]);
    expect(stub.requestsFor('/v1/inbox/items')).toHaveLength(before + 1);
    expect(client.getSnapshot().items).toHaveLength(4);
  });

  test('fetchMore does not toggle isLoading', async () => {
    const stub = createStubServer();
    for (let i = 0; i < 6; i += 1) {
      stub.addNotification();
    }
    const client = makeClient(stub, { pageSize: 2 });
    await connectAndLoad(client, stub);

    const loadingStates: boolean[] = [];
    client.subscribe(() => {
      loadingStates.push(client.getSnapshot().isLoading);
    });
    await client.fetchMore();
    expect(loadingStates.every((state) => !state)).toBe(true);
  });

  test('a refresh after deep pagination prepends new items without collapsing the tail', async () => {
    const stub = createStubServer();
    for (let i = 0; i < 8; i += 1) {
      stub.addNotification();
    }
    const client = makeClient(stub, { pageSize: 3 });
    await connectAndLoad(client, stub);
    await client.fetchMore();
    expect(client.getSnapshot().items).toHaveLength(6);

    const newest = stub.addNotification({ payload: { title: 'breaking' } });
    stub.emitHint();
    await vi.waitFor(() => {
      expect(client.getSnapshot().items).toHaveLength(7);
    });

    const snapshot = client.getSnapshot();
    expect(must(snapshot.items[0]).id).toBe(newest.id);
    expect(new Set(snapshot.items.map((item) => item.id)).size).toBe(7);
    expect(snapshot.hasMore).toBe(true);

    // Deep pagination continues from the kept tail, not from page one.
    await client.fetchMore();
    expect(client.getSnapshot().items).toHaveLength(9);
    expect(client.getSnapshot().hasMore).toBe(false);
  });

  test('a gap larger than one page resets the list instead of leaving a hole', async () => {
    const stub = createStubServer();
    for (let i = 0; i < 6; i += 1) {
      stub.addNotification();
    }
    const client = makeClient(stub, { pageSize: 3 });
    await connectAndLoad(client, stub);
    await client.fetchMore();
    expect(client.getSnapshot().items).toHaveLength(6);
    expect(client.getSnapshot().hasMore).toBe(false);

    // More than a page arrives while no hints are delivered (offline gap).
    for (let i = 0; i < 5; i += 1) {
      stub.addNotification();
    }
    stub.emitHint();
    await vi.waitFor(() => {
      expect(client.getSnapshot().isLoading).toBe(false);
      // Zero overlap with the stale list: the list resets to page one.
      expect(client.getSnapshot().items).toHaveLength(3);
    });
    expect(client.getSnapshot().hasMore).toBe(true);

    for (let i = 0; i < 5 && client.getSnapshot().hasMore; i += 1) {
      await client.fetchMore();
    }
    const snapshot = client.getSnapshot();
    expect(snapshot.hasMore).toBe(false);
    expect(snapshot.items).toHaveLength(11);
    expect(new Set(snapshot.items.map((item) => item.id)).size).toBe(11);
    const occurredAts = snapshot.items.map((item) => item.occurredAt);
    expect([...occurredAts].sort().reverse()).toEqual(occurredAts);
  });
});

describe('preferences', () => {
  test('round-trips explicit rows, where enabled=true deletes the row', async () => {
    const stub = createStubServer();
    const client = makeClient(stub);
    await connectAndLoad(client, stub);

    expect(await client.getPreferences()).toEqual([]);

    const disabled = await client.setPreferences([
      { category: 'marketing', channel: 'in_app', enabled: false },
    ]);
    expect(disabled).toEqual([{ category: 'marketing', channel: 'in_app', enabled: false }]);

    const enabled = await client.setPreferences([
      { category: 'marketing', channel: 'in_app', enabled: true },
    ]);
    expect(enabled).toEqual([]);
  });

  test('a successful write triggers a conditional list refresh', async () => {
    const stub = createStubServer();
    const client = makeClient(stub);
    await connectAndLoad(client, stub);

    const before = stub.requestsFor('/v1/inbox/items').length;
    await client.setPreferences([{ category: 'noise', channel: 'in_app', enabled: false }]);
    await vi.waitFor(() => {
      expect(stub.requestsFor('/v1/inbox/items').length).toBe(before + 1);
    });
  });

  test('a failed write rejects with the server code and surfaces on the error channel', async () => {
    const stub = createStubServer();
    const client = makeClient(stub);
    await connectAndLoad(client, stub);

    stub.failNext('PUT', '/v1/inbox/preferences', { status: 400, code: 'invalid_request' });
    await expect(
      client.setPreferences([{ category: 'x', channel: 'in_app', enabled: false }]),
    ).rejects.toMatchObject({ code: 'invalid_request', status: 400 });
    expect(client.getSnapshot().error?.code).toBe('invalid_request');
  });
});

describe('snapshot immutability', () => {
  test('every change produces a new snapshot object', async () => {
    const stub = createStubServer();
    stub.addNotification();
    const client = makeClient(stub);

    const seen: InboxSnapshot[] = [];
    client.subscribe(() => {
      seen.push(client.getSnapshot());
    });
    await connectAndLoad(client, stub);
    await client.markAllRead();

    expect(seen.length).toBeGreaterThan(2);
    for (let i = 1; i < seen.length; i += 1) {
      expect(seen[i]).not.toBe(seen[i - 1]);
    }
  });
});
