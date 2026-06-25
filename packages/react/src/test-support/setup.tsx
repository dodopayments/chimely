import type { ChimelyClientConfig } from '@chimely/client';
import { ChimelyClient } from '@chimely/client';
import { vi } from 'vitest';
import type { StubServer } from '../../../client/src/test-support/stub-server';
import { createStubServer } from '../../../client/src/test-support/stub-server';

export type { StubServer };
export { createStubServer };

export function makeClient(
  stub: StubServer,
  config: Partial<ChimelyClientConfig> = {},
): ChimelyClient {
  return new ChimelyClient({
    serverUrl: 'https://chimely.test',
    environment: stub.environment,
    subscriberId: stub.subscriberId,
    fetchFn: stub.fetchFn,
    createEventSource: stub.createEventSource,
    ...config,
  });
}

export async function loadClient(client: ChimelyClient, stub: StubServer): Promise<void> {
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
