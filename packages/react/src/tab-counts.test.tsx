import { act, renderHook, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { describe, expect, test } from 'vitest';
import { ChimelyProvider } from './context';
import { useExactTabCounts } from './tab-counts';
import { createStubServer, makeClient } from './test-support/setup';

describe('useExactTabCounts', () => {
  test('ignores a response for a previous tab configuration', async () => {
    const stub = createStubServer();
    stub.addNotification({ category: 'billing.alerts' });
    stub.addNotification({ category: 'security' });
    stub.addNotification({ category: 'security' });

    let releaseFirstRequest: () => void = () => {};
    const firstRequestGate = new Promise<void>((resolve) => {
      releaseFirstRequest = resolve;
    });
    let finishFirstRequest: () => void = () => {};
    const firstRequestFinished = new Promise<void>((resolve) => {
      finishFirstRequest = resolve;
    });
    let filteredRequestCount = 0;
    const client = makeClient(stub, {
      fetchFn: async (input, init) => {
        const url =
          input instanceof URL ? input.href : typeof input === 'string' ? input : input.url;
        if (new URL(url).pathname === '/v1/inbox/counts' && init?.method === 'POST') {
          filteredRequestCount += 1;
          if (filteredRequestCount === 1) {
            await firstRequestGate;
            const response = await stub.fetchFn(input, init);
            finishFirstRequest();
            return response;
          }
        }
        return stub.fetchFn(input, init);
      },
    });
    const wrapper = ({ children }: { children: ReactNode }) => (
      <ChimelyProvider client={client}>{children}</ChimelyProvider>
    );
    const refreshSignal: never[] = [];
    const { result, rerender } = renderHook(
      ({ category }: { category: string }) =>
        useExactTabCounts([{ categories: [category] }], refreshSignal),
      { initialProps: { category: 'billing.alerts' }, wrapper },
    );

    await waitFor(() => expect(filteredRequestCount).toBe(1));
    rerender({ category: 'security' });
    await waitFor(() => expect(result.current.counts.get(0)).toBe(2));
    expect(filteredRequestCount).toBe(2);

    await act(async () => {
      releaseFirstRequest();
      await firstRequestFinished;
    });
    expect(result.current.counts.get(0)).toBe(2);
    expect(filteredRequestCount).toBe(2);
  });
});
