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

describe('lastRefreshNewItemIds', () => {
  test('the initial load reports every first-page id as new', async () => {
    const stub = createStubServer();
    const a = stub.addNotification();
    const b = stub.addNotification();
    const client = makeClient(stub);
    await connectAndLoad(client, stub);

    const ids = client.getSnapshot().lastRefreshNewItemIds;
    expect(new Set(ids)).toEqual(new Set([a.id, b.id]));
  });

  test('a 304 refresh keeps the previous array untouched', async () => {
    const stub = createStubServer();
    stub.addNotification();
    const client = makeClient(stub);
    await connectAndLoad(client, stub);

    const before = client.getSnapshot().lastRefreshNewItemIds;
    await client.refresh();
    expect(client.getSnapshot().lastRefreshNewItemIds).toBe(before);
  });

  test('fetchMore leaves it untouched', async () => {
    const stub = createStubServer();
    for (let i = 0; i < 5; i += 1) {
      stub.addNotification();
    }
    const client = makeClient(stub, { pageSize: 3 });
    await connectAndLoad(client, stub);

    const before = client.getSnapshot().lastRefreshNewItemIds;
    await client.fetchMore();
    expect(client.getSnapshot().items).toHaveLength(5);
    expect(client.getSnapshot().lastRefreshNewItemIds).toBe(before);
  });

  test('a hint-driven prepend lists exactly the prepended ids', async () => {
    const stub = createStubServer();
    for (let i = 0; i < 8; i += 1) {
      stub.addNotification();
    }
    const client = makeClient(stub, { pageSize: 3 });
    await connectAndLoad(client, stub);
    await client.fetchMore();
    expect(client.getSnapshot().items).toHaveLength(6);

    const newest = stub.addNotification();
    stub.emitHint();
    await vi.waitFor(() => {
      expect(client.getSnapshot().items).toHaveLength(7);
    });
    expect(client.getSnapshot().lastRefreshNewItemIds).toEqual([newest.id]);
  });

  test('a non-contiguous reset lists the whole page as new', async () => {
    const stub = createStubServer();
    for (let i = 0; i < 6; i += 1) {
      stub.addNotification();
    }
    const client = makeClient(stub, { pageSize: 3 });
    await connectAndLoad(client, stub);
    await client.fetchMore();
    expect(client.getSnapshot().hasMore).toBe(false);

    for (let i = 0; i < 5; i += 1) {
      stub.addNotification();
    }
    stub.emitHint();
    await vi.waitFor(() => {
      expect(client.getSnapshot().items).toHaveLength(3);
    });
    const snapshot = client.getSnapshot();
    expect(snapshot.lastRefreshNewItemIds).toEqual(snapshot.items.map((item) => item.id));
  });
});
