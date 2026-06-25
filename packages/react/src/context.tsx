import type { ChimelyClientConfig } from '@chimely/client';
import { ChimelyClient } from '@chimely/client';
import type { ReactNode } from 'react';
import { createContext, useContext, useEffect, useState } from 'react';

export const ChimelyContext = createContext<ChimelyClient | null>(null);

/**
 * Provides one shared ChimelyClient to all hooks. Either `client` or `config`
 * is set. A passed `client` is caller-owned. A passed `config` is constructed
 * here, connected on mount, and closed on unmount.
 */
export interface ChimelyProviderProps {
  client?: ChimelyClient;
  config?: ChimelyClientConfig;
  children?: ReactNode;
}

export function ChimelyProvider(props: ChimelyProviderProps): ReactNode {
  const { client, config, children } = props;
  // The owned client is constructed once per provider instance. Later
  // changes to the config prop do not rebuild it.
  const [owned] = useState(() => (client || !config ? null : new ChimelyClient(config)));
  const value = client ?? owned;
  if (!value) {
    throw new Error('ChimelyProvider requires a client or a config prop');
  }
  useEffect(() => {
    if (client || !owned) {
      return undefined;
    }
    owned.connect();
    return () => {
      owned.close();
    };
  }, [client, owned]);
  return <ChimelyContext.Provider value={value}>{children}</ChimelyContext.Provider>;
}

/** The provider's client. Throws outside a <ChimelyProvider>. */
export function useChimelyClient(): ChimelyClient {
  const client = useContext(ChimelyContext);
  if (!client) {
    throw new Error('useChimelyClient must be called inside a <ChimelyProvider>');
  }
  return client;
}
