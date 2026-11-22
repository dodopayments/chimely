import { describe, expect, test } from 'vitest';
import { ChimelyClient } from './client';
import { createStubServer } from './test-support/stub-server';

describe('filtered counts', () => {
  test('posts ordered category filters and returns exact unread counts', async () => {
    const stub = createStubServer();
    stub.addNotification({ category: 'billing' });
    stub.addBroadcast({ category: 'refund' });
    stub.addNotification({ category: 'billing', read: true });
    stub.addNotification({ category: 'security', archived: true });
    stub.addNotification({ category: 'noise' });
    stub.setPreferenceRow({ category: 'noise', channel: 'in_app', enabled: false });
    const client = new ChimelyClient({
      serverUrl: 'https://chimely.test',
      environment: stub.environment,
      subscriberId: stub.subscriberId,
      fetchFn: stub.fetchFn,
      createEventSource: stub.createEventSource,
    });

    const counts = await client.getFilteredCounts([
      { categories: ['billing'] },
      { categories: ['billing', 'refund'] },
      { categories: ['security'] },
      { categories: ['noise'] },
      { categories: ['missing'] },
    ]);

    expect(counts).toEqual([
      { unread: 1 },
      { unread: 2 },
      { unread: 0 },
      { unread: 0 },
      { unread: 0 },
    ]);
    const request = stub.requestsFor('/v1/inbox/counts')[0];
    expect(request?.method).toBe('POST');
    expect(request?.body).toEqual({
      filters: [
        { categories: ['billing'] },
        { categories: ['billing', 'refund'] },
        { categories: ['security'] },
        { categories: ['noise'] },
        { categories: ['missing'] },
      ],
    });
  });
});
