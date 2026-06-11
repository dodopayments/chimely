import type {
  ConnectionStatus,
  DronteClient,
  InboxCounts,
  InboxItem,
  InboxItemId,
  InboxItemSource,
  InboxSnapshot,
  Preference,
  WellKnownPayload,
} from '@dronte/client';
import { DronteError } from '@dronte/client';
import { useCallback, useEffect, useState, useSyncExternalStore } from 'react';
import { useDronteClient } from './context';

function useTypedClient<TPayload>(): DronteClient<TPayload> {
  return useDronteClient() as unknown as DronteClient<TPayload>;
}

function useInboxSnapshot<TPayload>(): InboxSnapshot<TPayload> {
  const client = useTypedClient<TPayload>();
  const subscribe = useCallback((listener: () => void) => client.subscribe(listener), [client]);
  return useSyncExternalStore(
    subscribe,
    () => client.getSnapshot(),
    () => client.getSnapshot(),
  );
}

function asDronteError(cause: unknown): DronteError {
  if (cause instanceof DronteError) {
    return cause;
  }
  const message = cause instanceof Error ? cause.message : 'request failed';
  return new DronteError(message, { code: 'network', cause });
}

export interface UseNotificationsOptions {
  /** Override the client's pageSize for this consumer. */
  pageSize?: number;
}

export interface UseNotificationsResult<TPayload = WellKnownPayload> {
  items: ReadonlyArray<InboxItem<TPayload>>;
  isLoading: boolean;
  error: DronteError | null;
  hasMore: boolean;
  fetchMore: () => Promise<void>;
  refresh: () => Promise<void>;
  markRead: (item: { id: InboxItemId; source: InboxItemSource }) => Promise<void>;
  markAllRead: () => Promise<void>;
}

/** Headless merged-inbox list. */
export function useNotifications<TPayload = WellKnownPayload>(
  options?: UseNotificationsOptions,
): UseNotificationsResult<TPayload> {
  const client = useTypedClient<TPayload>();
  const snapshot = useInboxSnapshot<TPayload>();
  const pageSize = options?.pageSize;
  const fetchMore = useCallback(
    () => client.fetchMore(pageSize === undefined ? undefined : { limit: pageSize }),
    [client, pageSize],
  );
  const refresh = useCallback(() => client.refresh(), [client]);
  const markRead = useCallback(
    (item: { id: InboxItemId; source: InboxItemSource }) => client.markRead(item),
    [client],
  );
  const markAllRead = useCallback(() => client.markAllRead(), [client]);
  return {
    items: snapshot.items,
    isLoading: snapshot.isLoading,
    error: snapshot.error,
    hasMore: snapshot.hasMore,
    fetchMore,
    refresh,
    markRead,
    markAllRead,
  };
}

export interface UseCountResult {
  count: number;
  isLoading: boolean;
  error: DronteError | null;
}

/** Live unread count (list styling / "N unread" copy). */
export function useUnreadCount(): UseCountResult {
  const snapshot = useInboxSnapshot<WellKnownPayload>();
  return { count: snapshot.counts.unread, isLoading: snapshot.isLoading, error: snapshot.error };
}

/** Live unseen count (the bell badge). Cleared by markAllSeen. */
export function useUnseenCount(): UseCountResult {
  const snapshot = useInboxSnapshot<WellKnownPayload>();
  return { count: snapshot.counts.unseen, isLoading: snapshot.isLoading, error: snapshot.error };
}

export interface UsePreferencesResult {
  /** Explicit rows only — absence means enabled. */
  preferences: ReadonlyArray<Preference>;
  setPreferences: (preferences: Preference[]) => Promise<void>;
  isLoading: boolean;
  error: DronteError | null;
}

function applyWrites(rows: ReadonlyArray<Preference>, writes: Preference[]): Preference[] {
  let next = [...rows];
  for (const write of writes) {
    next = next.filter(
      (row) => !(row.category === write.category && row.channel === write.channel),
    );
    // Enabled is the default, expressed by the absence of an explicit row.
    if (!write.enabled) {
      next.push(write);
    }
  }
  return next;
}

export function usePreferences(): UsePreferencesResult {
  const client = useDronteClient();
  const [rows, setRows] = useState<ReadonlyArray<Preference> | null>(null);
  const [error, setError] = useState<DronteError | null>(null);

  useEffect(() => {
    let active = true;
    client.getPreferences().then(
      (loaded) => {
        if (active) {
          setRows(loaded);
          setError(null);
        }
      },
      (cause: unknown) => {
        if (active) {
          setError(asDronteError(cause));
        }
      },
    );
    return () => {
      active = false;
    };
  }, [client]);

  const setPreferences = useCallback(
    async (writes: Preference[]) => {
      const previous = rows;
      setRows(applyWrites(previous ?? [], writes));
      try {
        const next = await client.setPreferences(writes);
        setRows(next);
        setError(null);
      } catch (cause) {
        setRows(previous);
        setError(asDronteError(cause));
      }
    },
    [client, rows],
  );

  return {
    preferences: rows ?? [],
    setPreferences,
    isLoading: rows === null && error === null,
    error,
  };
}

export interface UseInboxResult<TPayload = WellKnownPayload> {
  items: ReadonlyArray<InboxItem<TPayload>>;
  counts: InboxCounts;
  status: ConnectionStatus;
  hasMore: boolean;
  isLoading: boolean;
  error: DronteError | null;
  fetchMore: () => Promise<void>;
  refresh: () => Promise<void>;
  markRead: (item: { id: InboxItemId; source: InboxItemSource }) => Promise<void>;
  markAllRead: () => Promise<void>;
  markAllSeen: () => Promise<void>;
}

/**
 * Convenience superset of the snapshot plus every action, for fully custom
 * inbox UIs. Additive on top of the frozen contract surface.
 */
export function useInbox<TPayload = WellKnownPayload>(): UseInboxResult<TPayload> {
  const client = useTypedClient<TPayload>();
  const snapshot = useInboxSnapshot<TPayload>();
  const fetchMore = useCallback(() => client.fetchMore(), [client]);
  const refresh = useCallback(() => client.refresh(), [client]);
  const markRead = useCallback(
    (item: { id: InboxItemId; source: InboxItemSource }) => client.markRead(item),
    [client],
  );
  const markAllRead = useCallback(() => client.markAllRead(), [client]);
  const markAllSeen = useCallback(() => client.markAllSeen(), [client]);
  return {
    items: snapshot.items,
    counts: snapshot.counts,
    status: snapshot.status,
    hasMore: snapshot.hasMore,
    isLoading: snapshot.isLoading,
    error: snapshot.error,
    fetchMore,
    refresh,
    markRead,
    markAllRead,
    markAllSeen,
  };
}
