import { render, screen, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, describe, expect, test, vi } from 'vitest';
import { DronteProvider, useDronteClient } from './context';
import { useNotifications } from './hooks';
import { createStubServer, loadClient, makeClient } from './test-support/setup';

function Probe(): ReactNode {
  const { items, isLoading } = useNotifications();
  return <output>{isLoading ? 'loading' : `items:${items.length}`}</output>;
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe('DronteProvider', () => {
  test('config mode constructs a client, connects on mount, and closes on unmount', async () => {
    const stub = createStubServer();
    stub.addNotification();
    const { unmount } = render(
      <DronteProvider
        config={{
          serverUrl: 'https://dronte.test',
          environment: stub.environment,
          subscriberId: stub.subscriberId,
          fetchFn: stub.fetchFn,
          createEventSource: stub.createEventSource,
        }}
      >
        <Probe />
      </DronteProvider>,
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
      <DronteProvider client={client}>
        <Probe />
      </DronteProvider>,
    );
    await waitFor(() => {
      expect(screen.getByText('items:1')).toBeDefined();
    });

    unmount();
    expect(client.getSnapshot().status).toBe('connected');
    expect(stub.stream().closed).toBe(false);
  });

  test('useDronteClient throws outside a provider', () => {
    vi.spyOn(console, 'error').mockImplementation(() => {});
    function Bare(): ReactNode {
      useDronteClient();
      return null;
    }
    expect(() => render(<Bare />)).toThrow(/inside a <DronteProvider>/);
  });

  test('provider without client or config throws', () => {
    vi.spyOn(console, 'error').mockImplementation(() => {});
    expect(() => render(<DronteProvider>x</DronteProvider>)).toThrow(/client or a config/);
  });
});
