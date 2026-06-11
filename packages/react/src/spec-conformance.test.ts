/**
 * Compile-time conformance against the '@dronte/react' module declaration
 * in specs/sdk-api.d.ts (frozen at contract-v1, additive-only). Shapes are
 * transcribed because the spec's ambient module would clash with the real
 * package. Drift fails `tsc --noEmit`, not the runtime test.
 */

import type {
  DronteClient,
  DronteClientConfig,
  DronteError,
  InboxItem,
  InboxItemId,
  InboxItemSource,
  Preference,
  WellKnownPayload,
} from '@dronte/client';
import type { ReactNode } from 'react';
import { expectTypeOf, test } from 'vitest';
import type {
  DronteProviderProps,
  InboxAppearance,
  InboxLocalization,
  InboxProps,
  InboxSlot,
  UseCountResult,
  UseNotificationsOptions,
  UseNotificationsResult,
  UsePreferencesResult,
} from './index';
import {
  DronteProvider,
  Inbox,
  useDronteClient,
  useNotifications,
  usePreferences,
  useUnreadCount,
  useUnseenCount,
} from './index';

test('provider surface matches the frozen contract', () => {
  expectTypeOf<DronteProviderProps>().toEqualTypeOf<{
    client?: DronteClient;
    config?: DronteClientConfig;
    children?: ReactNode;
  }>();
  expectTypeOf(DronteProvider).toEqualTypeOf<(props: DronteProviderProps) => ReactNode>();
  expectTypeOf(useDronteClient).toEqualTypeOf<() => DronteClient>();
});

test('hook surfaces match the frozen contract', () => {
  expectTypeOf<UseNotificationsOptions>().toEqualTypeOf<{ pageSize?: number }>();
  expectTypeOf<UseNotificationsResult>().toEqualTypeOf<{
    items: ReadonlyArray<InboxItem>;
    isLoading: boolean;
    error: DronteError | null;
    hasMore: boolean;
    fetchMore: () => Promise<void>;
    refresh: () => Promise<void>;
    markRead: (item: { id: InboxItemId; source: InboxItemSource }) => Promise<void>;
    markAllRead: () => Promise<void>;
  }>();
  expectTypeOf(useNotifications).toExtend<
    (options?: UseNotificationsOptions) => UseNotificationsResult
  >();
  expectTypeOf<UseNotificationsResult<{ amount: number }>['items']>().toEqualTypeOf<
    ReadonlyArray<InboxItem<{ amount: number }>>
  >();

  expectTypeOf<UseCountResult>().toEqualTypeOf<{
    count: number;
    isLoading: boolean;
    error: DronteError | null;
  }>();
  expectTypeOf(useUnreadCount).toEqualTypeOf<() => UseCountResult>();
  expectTypeOf(useUnseenCount).toEqualTypeOf<() => UseCountResult>();

  expectTypeOf<UsePreferencesResult>().toEqualTypeOf<{
    preferences: ReadonlyArray<Preference>;
    setPreferences: (preferences: Preference[]) => Promise<void>;
    isLoading: boolean;
    error: DronteError | null;
  }>();
  expectTypeOf(usePreferences).toEqualTypeOf<() => UsePreferencesResult>();
});

test('<Inbox /> surface matches the frozen contract', () => {
  expectTypeOf<InboxSlot>().toEqualTypeOf<
    | 'root'
    | 'bell'
    | 'badge'
    | 'popover'
    | 'header'
    | 'list'
    | 'item'
    | 'itemUnread'
    | 'empty'
    | 'footer'
    | 'preferences'
  >();

  expectTypeOf<InboxAppearance>().toEqualTypeOf<{
    variables?: {
      colorPrimary?: string;
      colorBackground?: string;
      colorForeground?: string;
      colorMuted?: string;
      colorBadge?: string;
      borderRadius?: string;
      fontFamily?: string;
      fontSize?: string;
      [customProperty: string]: string | undefined;
    };
    classNames?: Partial<Record<InboxSlot, string>>;
  }>();

  // No index signature on purpose. Equality would fail if one appeared.
  expectTypeOf<InboxLocalization>().toEqualTypeOf<{
    emptyTitle: string;
    emptyBody: string;
    markAllRead: string;
    preferencesTitle: string;
  }>();

  expectTypeOf<InboxProps>().toEqualTypeOf<{
    serverUrl?: string;
    environment?: string;
    subscriberId?: string;
    subscriberHash?: string;
    backoff?: DronteClientConfig['backoff'];
    appearance?: InboxAppearance;
    localization?: Partial<InboxLocalization>;
    placement?: 'bottom-start' | 'bottom-end' | 'top-start' | 'top-end';
    preferencesPanel?: boolean;
    // biome-ignore lint/suspicious/noConfusingVoidType: frozen contract type (specs/sdk-api.d.ts)
    onItemClick?: (item: InboxItem) => boolean | void;
    renderItem?: (ctx: { item: InboxItem; markRead: () => Promise<void> }) => ReactNode;
    renderBell?: (ctx: { unseenCount: number; open: boolean }) => ReactNode;
    renderEmpty?: () => ReactNode;
  }>();
  expectTypeOf<InboxProps<{ amount: number }>['renderItem']>().toEqualTypeOf<
    | ((ctx: { item: InboxItem<{ amount: number }>; markRead: () => Promise<void> }) => ReactNode)
    | undefined
  >();

  expectTypeOf(Inbox).toExtend<(props: InboxProps) => ReactNode>();
  expectTypeOf(Inbox<WellKnownPayload>).toEqualTypeOf<
    (props: InboxProps<WellKnownPayload>) => ReactNode
  >();
});
