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

describe('archive', () => {
  test('optimistically removes the item from the default view and adjusts unread', async () => {
    const stub = createStubServer();
    const item = stub.addNotification();
    stub.addNotification();
    const client = makeClient(stub);
    await connectAndLoad(client, stub);
    expect(client.getSnapshot().counts.unread).toBe(2);

    await client.archive({ id: item.id as never, source: 'notification' });
    const snapshot = client.getSnapshot();
    expect(snapshot.items.find((i) => i.id === item.id)).toBeUndefined();
    expect(snapshot.counts.unread).toBe(1);
    expect(stub.requestsFor(`/v1/inbox/notifications/${item.id}/archive`)).toHaveLength(1);
  });

  test('rolls back on failure', async () => {
    const stub = createStubServer();
    const item = stub.addNotification();
    const client = makeClient(stub);
    await connectAndLoad(client, stub);

    stub.failNext('POST', '/archive', { status: 500, code: 'internal' });
    await client.archive({ id: item.id as never, source: 'notification' });
    await vi.waitFor(() => {
      expect(client.getSnapshot().error).not.toBeNull();
    });
    expect(client.getSnapshot().items).toHaveLength(1);
    expect(client.getSnapshot().counts.unread).toBe(1);
  });

  test('unarchive removes the item from the archived view and restores unread', async () => {
    const stub = createStubServer();
    const item = stub.addNotification({ archived: true });
    stub.addNotification();
    const client = makeClient(stub);
    await connectAndLoad(client, stub);

    await client.setFilter('archived');
    expect(client.getSnapshot().items.map((i) => i.id)).toEqual([item.id]);
    expect(client.getSnapshot().items[0]?.archived).toBe(true);

    await client.unarchive({ id: item.id as never, source: 'notification' });
    const snapshot = client.getSnapshot();
    expect(snapshot.items).toHaveLength(0);
    expect(snapshot.counts.unread).toBe(2);
    expect(stub.requestsFor(`/v1/inbox/notifications/${item.id}/unarchive`)).toHaveLength(1);
  });

  test('archiveAll clears the default view and zeroes unread', async () => {
    const stub = createStubServer();
    stub.addNotification();
    stub.addBroadcast();
    const client = makeClient(stub);
    await connectAndLoad(client, stub);

    await client.archiveAll();
    expect(client.getSnapshot().items).toHaveLength(0);
    expect(client.getSnapshot().counts.unread).toBe(0);
    expect(stub.requestsFor('/v1/inbox/archive-all')).toHaveLength(1);
  });

  test('archiveRead posts and converges on the next refresh', async () => {
    const stub = createStubServer();
    stub.addNotification({ read: true });
    stub.addNotification();
    const client = makeClient(stub);
    await connectAndLoad(client, stub);
    expect(client.getSnapshot().items).toHaveLength(2);

    await client.archiveRead();
    expect(stub.requestsFor('/v1/inbox/archive-read')).toHaveLength(1);
    // No optimistic patch: the stub applied it server-side, the refresh
    // (hint-driven in production) converges the snapshot.
    stub.emitHint();
    await vi.waitFor(() => {
      expect(client.getSnapshot().items).toHaveLength(1);
    });
    expect(client.getSnapshot().items[0]?.read).toBe(false);
  });
});
