import { render, screen, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, describe, expect, test, vi } from 'vitest';
import { ChimelyProvider, useChimelyClient } from './context';
import { useNotifications } from './hooks';
import { createStubServer, loadClient, makeClient } from './test-support/setup';

function Probe(): ReactNode {
  const { items, isLoading } = useNotifications();
  return <output>{isLoading ? 'loading' : `items:${items.length}`}</output>;
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe('ChimelyProvider', () => {
  test('config mode constructs a client, connects on mount, and closes on unmount', async () => {
    const stub = createStubServer();
    stub.addNotification();
    const { unmount } = render(
      <ChimelyProvider
        config={{
          serverUrl: 'https://chimely.test',
          environment: stub.environment,
          subscriberId: stub.subscriberId,
          fetchFn: stub.fetchFn,
          createEventSource: stub.createEventSource,
        }}
      >
        <Probe />
      </ChimelyProvider>,
    );

    expect(stub.sources).toHaveLength(1);
    stub.openStream();
    await waitFor(() => {
      expect(screen.getByText('items:1')).toBeDefined();
    });

    unmount();
    expect(stub.stream().closed).toBe(true);
  });

  test('client mode leaves the lifecycle to the caller', async () => {
    const stub = createStubServer();
    stub.addNotification();
    const client = makeClient(stub);
    await loadClient(client, stub);

    const { unmount } = render(
      <ChimelyProvider client={client}>
        <Probe />
      </ChimelyProvider>,
    );
    await waitFor(() => {
      expect(screen.getByText('items:1')).toBeDefined();
    });

    unmount();
    expect(client.getSnapshot().status).toBe('connected');
    expect(stub.stream().closed).toBe(false);
  });

  test('useChimelyClient throws outside a provider', () => {
    vi.spyOn(console, 'error').mockImplementation(() => {});
    function Bare(): ReactNode {
      useChimelyClient();
      return null;
    }
    expect(() => render(<Bare />)).toThrow(/inside a <ChimelyProvider>/);
  });

  test('provider without client or config throws', () => {
    vi.spyOn(console, 'error').mockImplementation(() => {});
    expect(() => render(<ChimelyProvider>x</ChimelyProvider>)).toThrow(/client or a config/);
  });
});
