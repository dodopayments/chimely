/**
 * Compile-time conformance against the '@dronte/client' module declaration
 * in specs/sdk-api.d.ts (frozen at contract-v1, additive-only). The spec
 * cannot be imported directly (its ambient module would clash with the real
 * package), so the shapes are transcribed. Drift fails `tsc --noEmit`, not
 * the runtime test.
 */

import { expectTypeOf, test } from 'vitest';
import type {
  BackoffConfig,
  BroadcastId,
  ConnectionStatus,
  DronteClientConfig,
  EventSourceLike,
  InboxCounts,
  InboxItem,
  InboxItemId,
  InboxItemSource,
  InboxSnapshot,
  NotificationId,
  Preference,
  WellKnownPayload,
} from './index';
import { DronteClient, type DronteError } from './index';

test('domain types match the frozen contract', () => {
  expectTypeOf<InboxItemSource>().toEqualTypeOf<'notification' | 'broadcast'>();
  expectTypeOf<NotificationId>().toEqualTypeOf<`notif_${string}`>();
  expectTypeOf<BroadcastId>().toEqualTypeOf<`bcast_${string}`>();
  expectTypeOf<InboxItemId>().toEqualTypeOf<NotificationId | BroadcastId>();
  expectTypeOf<ConnectionStatus>().toEqualTypeOf<
    'idle' | 'connecting' | 'connected' | 'reconnecting' | 'closed'
  >();

  expectTypeOf<WellKnownPayload>().toEqualTypeOf<{
    title?: string;
    body?: string;
    action_url?: string;
    icon_url?: string;
    [custom: string]: unknown;
  }>();

  expectTypeOf<InboxItem>().toEqualTypeOf<{
    id: InboxItemId;
    source: InboxItemSource;
    category: string;
    payload: WellKnownPayload;
    occurredAt: string;
    read: boolean;
  }>();
  expectTypeOf<InboxItem<{ amount: number }>['payload']>().toEqualTypeOf<{ amount: number }>();

  expectTypeOf<InboxCounts>().toEqualTypeOf<{ unread: number; unseen: number }>();
  expectTypeOf<Preference>().toEqualTypeOf<{
    category: string;
    channel: 'in_app';
    enabled: boolean;
  }>();
});

test('config types match the frozen contract', () => {
  expectTypeOf<BackoffConfig>().toEqualTypeOf<{
    initialDelayMs?: number;
    maxDelayMs?: number;
    multiplier?: number;
    jitter?: number;
    maxAttempts?: number;
  }>();

  expectTypeOf<DronteClientConfig>().toEqualTypeOf<{
    serverUrl: string;
    environment: string;
    subscriberId: string;
    subscriberHash?: string;
    backoff?: BackoffConfig;
    pageSize?: number;
    fetchFn?: typeof fetch;
    createEventSource?: (url: string) => EventSourceLike;
  }>();

  expectTypeOf<EventSourceLike['addEventListener']>().toEqualTypeOf<
    (
      type: 'open' | 'error' | string,
      listener: (event: { data?: string; lastEventId?: string }) => void,
    ) => void
  >();
  expectTypeOf<EventSourceLike['close']>().toEqualTypeOf<() => void>();
});

test('snapshot shape matches the frozen contract', () => {
  expectTypeOf<InboxSnapshot>().toEqualTypeOf<{
    items: ReadonlyArray<InboxItem>;
    counts: InboxCounts;
    status: ConnectionStatus;
    hasMore: boolean;
    isLoading: boolean;
    error: DronteError | null;
  }>();
  expectTypeOf<InboxSnapshot<{ amount: number }>['items']>().toEqualTypeOf<
    ReadonlyArray<InboxItem<{ amount: number }>>
  >();
});

test('DronteClient surface matches the frozen contract', () => {
  expectTypeOf(DronteClient).constructorParameters.toEqualTypeOf<[DronteClientConfig]>();

  expectTypeOf<DronteClient['connect']>().toEqualTypeOf<() => void>();
  expectTypeOf<DronteClient['close']>().toEqualTypeOf<() => void>();
  expectTypeOf<DronteClient['getSnapshot']>().toEqualTypeOf<() => InboxSnapshot>();
  expectTypeOf<DronteClient['subscribe']>().toEqualTypeOf<(listener: () => void) => () => void>();
  // fetchMore gained an additive optional options bag. It must still satisfy
  // the frozen zero-argument signature.
  expectTypeOf<DronteClient['fetchMore']>().toExtend<() => Promise<void>>();
  expectTypeOf<DronteClient['refresh']>().toEqualTypeOf<() => Promise<void>>();
  expectTypeOf<DronteClient['markRead']>().toEqualTypeOf<
    (item: { id: InboxItemId; source: InboxItemSource }) => Promise<void>
  >();
  expectTypeOf<DronteClient['markAllRead']>().toEqualTypeOf<() => Promise<void>>();
  expectTypeOf<DronteClient['markAllSeen']>().toEqualTypeOf<() => Promise<void>>();
  expectTypeOf<DronteClient['getPreferences']>().toEqualTypeOf<() => Promise<Preference[]>>();
  expectTypeOf<DronteClient['setPreferences']>().toEqualTypeOf<
    (preferences: Preference[]) => Promise<Preference[]>
  >();

  expectTypeOf<DronteClient<{ amount: number }>['getSnapshot']>().toEqualTypeOf<
    () => InboxSnapshot<{ amount: number }>
  >();
});

test('DronteError matches the frozen contract', () => {
  expectTypeOf<DronteError>().toExtend<Error>();
  expectTypeOf<DronteError['code']>().toEqualTypeOf<string>();
  expectTypeOf<DronteError['status']>().toEqualTypeOf<number | undefined>();
});
