import type { DronteClientConfig } from '@dronte/client';
import { DronteClient } from '@dronte/client';
import type { ReactNode } from 'react';
import { createContext, useContext, useEffect, useState } from 'react';

export const DronteContext = createContext<DronteClient | null>(null);

/**
 * Provides one shared DronteClient to all hooks. Pass either a pre-built
 * `client` (you own its lifecycle) or `config` (the provider constructs,
 * connects on mount, closes on unmount).
 */
export interface DronteProviderProps {
  client?: DronteClient;
  config?: DronteClientConfig;
  children?: ReactNode;
}

export function DronteProvider(props: DronteProviderProps): ReactNode {
  const { client, config, children } = props;
  // The owned client is constructed once per provider instance. Later
  // changes to the config prop do not rebuild it.
  const [owned] = useState(() => (client || !config ? null : new DronteClient(config)));
  const value = client ?? owned;
  if (!value) {
    throw new Error('DronteProvider requires a client or a config prop');
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
  return <DronteContext.Provider value={value}>{children}</DronteContext.Provider>;
}

/** The provider's client. Throws outside a <DronteProvider>. */
export function useDronteClient(): DronteClient {
  const client = useContext(DronteContext);
  if (!client) {
    throw new Error('useDronteClient must be called inside a <DronteProvider>');
  }
  return client;
}
