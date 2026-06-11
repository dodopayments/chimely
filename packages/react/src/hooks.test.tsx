import { act, renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { describe, expect, test } from 'vitest';
import { DronteProvider } from './context';
import {
  useInbox,
  useNotifications,
  usePreferences,
  useUnreadCount,
  useUnseenCount,
} from './hooks';
import type { StubServer } from './test-support/setup';
import { createStubServer, loadClient, makeClient } from './test-support/setup';

async function loadedWrapper(stub: StubServer, clientConfig: { pageSize?: number } = {}) {
  const client = makeClient(stub, clientConfig);
  await loadClient(client, stub);
  const wrapper = ({ children }: { children?: ReactNode }) => (
    <DronteProvider client={client}>{children}</DronteProvider>
  );
  return { client, wrapper };
}

describe('useNotifications', () => {
  test('exposes the merged list and live updates from hints', async () => {
    const stub = createStubServer();
    stub.addNotification({ payload: { title: 'first' } });
    const { wrapper } = await loadedWrapper(stub);

    const { result } = renderHook(() => useNotifications(), { wrapper });
    expect(result.current.items).toHaveLength(1);
    expect(result.current.hasMore).toBe(false);
    expect(result.current.error).toBeNull();

    stub.addBroadcast({ payload: { title: 'second' } });
    act(() => {
      stub.emitHint();
    });
    await waitFor(() => {
      expect(result.current.items).toHaveLength(2);
    });
    expect(result.current.items[0]?.payload.title).toBe('second');
  });

  test('fetchMore pages with the per-consumer pageSize override', async () => {
    const stub = createStubServer();
    for (let i = 0; i < 7; i += 1) {
      stub.addNotification();
    }
    const { wrapper } = await loadedWrapper(stub, { pageSize: 2 });

    const { result } = renderHook(() => useNotifications({ pageSize: 3 }), { wrapper });
    expect(result.current.items).toHaveLength(2);
    expect(result.current.hasMore).toBe(true);

    await act(async () => {
      await result.current.fetchMore();
    });
    expect(result.current.items).toHaveLength(5);
    const lastListRequest = stub.requestsFor('/v1/inbox/items').at(-1);
    expect(lastListRequest?.search.get('limit')).toBe('3');
    expect(lastListRequest?.search.get('cursor')).toBeTruthy();
  });

  test('markRead flows through to the client optimistically', async () => {
    const stub = createStubServer();
    const item = stub.addNotification();
    const { wrapper } = await loadedWrapper(stub);

    const { result } = renderHook(() => useNotifications(), { wrapper });
    await act(async () => {
      await result.current.markRead({ id: item.id as `notif_${string}`, source: 'notification' });
    });
    expect(result.current.items[0]?.read).toBe(true);
    expect(stub.requestsFor(`/v1/inbox/notifications/${item.id}/read`)).toHaveLength(1);
  });
});

describe('count hooks', () => {
  test('useUnreadCount and useUnseenCount track the live counts', async () => {
    const stub = createStubServer();
    stub.addNotification();
    stub.addNotification({ read: true, seen: true });
    const { client, wrapper } = await loadedWrapper(stub);

    const unread = renderHook(() => useUnreadCount(), { wrapper });
    const unseen = renderHook(() => useUnseenCount(), { wrapper });
    expect(unread.result.current.count).toBe(1);
    expect(unseen.result.current.count).toBe(1);

    await act(async () => {
      await client.markAllSeen();
    });
    expect(unseen.result.current.count).toBe(0);
    expect(unread.result.current.count).toBe(1);
  });
});

describe('usePreferences', () => {
  test('loads explicit rows and writes optimistically', async () => {
    const stub = createStubServer();
    stub.setPreferenceRow({ category: 'noise', channel: 'in_app', enabled: false });
    const { wrapper } = await loadedWrapper(stub);

    const { result } = renderHook(() => usePreferences(), { wrapper });
    expect(result.current.isLoading).toBe(true);
    await waitFor(() => {
      expect(result.current.isLoading).toBe(false);
    });
    expect(result.current.preferences).toEqual([
      { category: 'noise', channel: 'in_app', enabled: false },
    ]);

    await act(async () => {
      await result.current.setPreferences([
        { category: 'noise', channel: 'in_app', enabled: true },
      ]);
    });
    expect(result.current.preferences).toEqual([]);
    expect(result.current.error).toBeNull();
  });

  test('a failed write rolls back and surfaces the error', async () => {
    const stub = createStubServer();
    const { wrapper } = await loadedWrapper(stub);

    const { result } = renderHook(() => usePreferences(), { wrapper });
    await waitFor(() => {
      expect(result.current.isLoading).toBe(false);
    });

    stub.failNext('PUT', '/v1/inbox/preferences', { status: 400, code: 'invalid_request' });
    await act(async () => {
      await result.current.setPreferences([
        { category: 'spam', channel: 'in_app', enabled: false },
      ]);
    });
    expect(result.current.preferences).toEqual([]);
    expect(result.current.error?.code).toBe('invalid_request');
  });
});

describe('useInbox', () => {
  test('exposes the full snapshot plus every action', async () => {
    const stub = createStubServer();
    stub.addNotification();
    const { wrapper } = await loadedWrapper(stub);

    const { result } = renderHook(() => useInbox(), { wrapper });
    expect(result.current.status).toBe('connected');
    expect(result.current.counts).toEqual({ unread: 1, unseen: 1 });
    expect(result.current.items).toHaveLength(1);

    await act(async () => {
      await result.current.markAllSeen();
    });
    expect(result.current.counts.unseen).toBe(0);
  });
});
