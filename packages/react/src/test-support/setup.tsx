import type { DronteClientConfig } from '@dronte/client';
import { DronteClient } from '@dronte/client';
import { vi } from 'vitest';
import type { StubServer } from '../../../client/src/test-support/stub-server';
import { createStubServer } from '../../../client/src/test-support/stub-server';

export type { StubServer };
export { createStubServer };

export function makeClient(
  stub: StubServer,
  config: Partial<DronteClientConfig> = {},
): DronteClient {
  return new DronteClient({
    serverUrl: 'https://dronte.test',
    environment: stub.environment,
    subscriberId: stub.subscriberId,
    fetchFn: stub.fetchFn,
    createEventSource: stub.createEventSource,
    ...config,
  });
}

export async function loadClient(client: DronteClient, stub: StubServer): Promise<void> {
  client.connect();
  stub.openStream();
  await vi.waitFor(() => {
    if (client.getSnapshot().isLoading) {
      throw new Error('still loading');
    }
    if (stub.requestsFor('/v1/inbox/counts').length === 0) {
      throw new Error('counts not fetched yet');
    }
  });
  await client.refresh();
}
