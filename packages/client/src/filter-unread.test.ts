import { describe, expect, test, vi } from 'vitest';
import { ChimelyClient } from './client';
import type { StubServer } from './test-support/stub-server';
import { createStubServer } from './test-support/stub-server';
import type { ChimelyClientConfig } from './types';

function makeClient(stub: StubServer, config: Partial<ChimelyClientConfig> = {}): ChimelyClient {
  return new ChimelyClient({
    serverUrl: 'https://chimely.test',
    environment: stub.environment,
    subscriberId: stub.subscriberId,
    fetchFn: stub.fetchFn,
    createEventSource: stub.createEventSource,
    ...config,
  });
}

async function connectAndLoad(client: ChimelyClient, stub: StubServer): Promise<void> {
  client.connect();
  stub.openStream();
  await vi.waitFor(() => {
    expect(client.getSnapshot().isLoading).toBe(false);
    expect(stub.requestsFor('/v1/inbox/counts').length).toBeGreaterThan(0);
  });
  await client.refresh();
}

describe('markUnread', () => {
  test('optimistically flips the item and bumps unread, then POSTs', async () => {
    const stub = createStubServer();
    const item = stub.addNotification({ read: true });
    stub.addNotification();
    const client = makeClient(stub);
    await connectAndLoad(client, stub);
    expect(client.getSnapshot().counts.unread).toBe(1);

    await client.markUnread({ id: item.id as never, source: 'notification' });
    const snapshot = client.getSnapshot();
    expect(snapshot.items.find((i) => i.id === item.id)?.read).toBe(false);
    expect(snapshot.counts.unread).toBe(2);
    expect(stub.requestsFor(`/v1/inbox/notifications/${item.id}/unread`)).toHaveLength(1);
  });

  test('routes broadcasts to the broadcast endpoint', async () => {
    const stub = createStubServer();
    const item = stub.addBroadcast({ read: true });
    const client = makeClient(stub);
    await connectAndLoad(client, stub);

    await client.markUnread({ id: item.id as never, source: 'broadcast' });
    expect(stub.requestsFor(`/v1/inbox/broadcasts/${item.id}/unread`)).toHaveLength(1);
  });

  test('rolls back on failure and surfaces the error', async () => {
    const stub = createStubServer();
    const item = stub.addNotification({ read: true });
    const client = makeClient(stub);
    await connectAndLoad(client, stub);

    stub.failNext('POST', '/unread', { status: 500, code: 'internal' });
    await client.markUnread({ id: item.id as never, source: 'notification' });
    await vi.waitFor(() => {
      expect(client.getSnapshot().error).not.toBeNull();
    });
    const snapshot = client.getSnapshot();
    expect(snapshot.items.find((i) => i.id === item.id)?.read).toBe(true);
    expect(snapshot.counts.unread).toBe(0);
  });
});

describe('setFilter', () => {
  test('switches the view, resets pagination, and refetches with the param', async () => {
    const stub = createStubServer();
    stub.addNotification({ read: true });
    const unreadItem = stub.addNotification();
    const client = makeClient(stub);
    await connectAndLoad(client, stub);
    expect(client.getSnapshot().items).toHaveLength(2);

    await client.setFilter('unread');
    const snapshot = client.getSnapshot();
    expect(snapshot.filter).toBe('unread');
    expect(snapshot.items.map((i) => i.id)).toEqual([unreadItem.id]);
    const filtered = stub
      .requestsFor('/v1/inbox/items')
      .filter((r) => r.search.get('filter') === 'unread');
    expect(filtered.length).toBeGreaterThan(0);

    // Unchanged filter is a no-op.
    const before = stub.requestsFor('/v1/inbox/items').length;
    await client.setFilter('unread');
    expect(stub.requestsFor('/v1/inbox/items')).toHaveLength(before);

    // Back to the default view restores everything.
    await client.setFilter('default');
    expect(client.getSnapshot().items).toHaveLength(2);
  });

  test('discards an in-flight fetchMore page from before a filter switch', async () => {
    const stub = createStubServer();
    stub.addNotification({ read: true });
    stub.addNotification({ read: true });
    stub.addNotification();
    stub.addNotification();
    stub.addNotification();
    let holdCursorPage: Promise<void> | null = null;
    const gatedFetch: typeof fetch = async (input, init) => {
      const url = new URL(String(input instanceof Request ? input.url : input));
      if (url.searchParams.has('cursor') && !url.searchParams.has('filter') && holdCursorPage) {
        await holdCursorPage;
      }
      return stub.fetchFn(input, init);
    };
    const client = makeClient(stub, { pageSize: 2, fetchFn: gatedFetch });
    await connectAndLoad(client, stub);
    expect(client.getSnapshot().items).toHaveLength(2);

    let release: () => void = () => {};
    holdCursorPage = new Promise<void>((resolve) => {
      release = resolve;
    });
    const stalePage = client.fetchMore();
    await client.setFilter('unread');
    release();
    await stalePage;

    // The default-view page two (which contains read items) must not land
    // in the unread view, and its cursor must not survive the switch.
    const snapshot = client.getSnapshot();
    expect(snapshot.filter).toBe('unread');
    expect(snapshot.items.every((i) => !i.read)).toBe(true);
    expect(snapshot.items).toHaveLength(2);

    await client.fetchMore();
    expect(client.getSnapshot().items).toHaveLength(3);
    expect(client.getSnapshot().items.every((i) => !i.read)).toBe(true);
    expect(client.getSnapshot().hasMore).toBe(false);
  });

  test('discards an in-flight refresh page from before a filter switch', async () => {
    const stub = createStubServer();
    stub.addNotification({ read: true });
    stub.addNotification();
    let holdFirstPage: Promise<void> | null = null;
    const gatedFetch: typeof fetch = async (input, init) => {
      const url = new URL(String(input instanceof Request ? input.url : input));
      if (url.pathname === '/v1/inbox/items' && !url.searchParams.has('filter') && holdFirstPage) {
        await holdFirstPage;
      }
      return stub.fetchFn(input, init);
    };
    const client = makeClient(stub, { fetchFn: gatedFetch });
    await connectAndLoad(client, stub);

    // Invalidate the stored ETag server-side so the gated refresh is a 200.
    stub.addNotification({ read: true });
    let release: () => void = () => {};
    holdFirstPage = new Promise<void>((resolve) => {
      release = resolve;
    });
    const staleRefresh = client.refresh();
    const switched = client.setFilter('unread');
    release();
    await staleRefresh;
    await switched;

    // The default-view first page must not merge into the unread view.
    const snapshot = client.getSnapshot();
    expect(snapshot.filter).toBe('unread');
    expect(snapshot.items.every((i) => !i.read)).toBe(true);
    expect(snapshot.items).toHaveLength(1);
    expect(snapshot.isLoading).toBe(false);
  });

  test('fetchMore carries the active filter', async () => {
    const stub = createStubServer();
    for (let i = 0; i < 5; i += 1) {
      stub.addNotification();
    }
    stub.addNotification({ read: true });
    const client = makeClient(stub, { pageSize: 2 });
    await connectAndLoad(client, stub);

    await client.setFilter('unread');
    expect(client.getSnapshot().items).toHaveLength(2);
    await client.fetchMore();
    const pages = stub
      .requestsFor('/v1/inbox/items')
      .filter((r) => r.search.get('cursor') !== null);
    expect(pages.every((r) => r.search.get('filter') === 'unread')).toBe(true);
    await client.fetchMore();
    expect(client.getSnapshot().items).toHaveLength(5);
    expect(client.getSnapshot().items.every((i) => !i.read)).toBe(true);
    expect(client.getSnapshot().hasMore).toBe(false);
  });
});
