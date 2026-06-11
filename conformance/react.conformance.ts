/**
 * Compile-time conformance of the BUILT @dronte/react (dist/index.d.ts)
 * against the frozen contract, renamed to @dronte-spec/react by
 * scripts/conformance-spec.mjs. Same rules as the client check, with one
 * wrinkle: the real DronteClient class has private fields, so the spec's
 * structural DronteClient is not assignable TO it. Wherever the client
 * class appears in a parameter position, the assertion substitutes the
 * implementation's class and checks the rest of the shape exactly.
 */

import type * as SpecClient from '@dronte-spec/client';
import type * as Spec from '@dronte-spec/react';
import type * as ImplClient from '../packages/client/dist/index.js';
import type * as Impl from '../packages/react/dist/index.js';

type Assert<T extends true> = T;
type Extends<A, B> = [A] extends [B] ? true : false;
type Mutual<A, B> = Extends<A, B> extends true ? Extends<B, A> : false;

// ----------------------------------------------------------------- provider
export type ProviderPropsShape = Assert<
  Mutual<Omit<Impl.DronteProviderProps, 'client'>, Omit<Spec.DronteProviderProps, 'client'>>
>;
export type ProviderPropsClient = Assert<
  Extends<Impl.DronteProviderProps['client'], Spec.DronteProviderProps['client']>
>;
type SpecProviderPropsWithImplClient = Omit<Spec.DronteProviderProps, 'client'> & {
  client?: ImplClient.DronteClient;
};
export type Provider = Assert<
  Extends<
    typeof Impl.DronteProvider,
    (props: SpecProviderPropsWithImplClient) => ReturnType<typeof Spec.DronteProvider>
  >
>;
export type UseClient = Assert<Extends<typeof Impl.useDronteClient, typeof Spec.useDronteClient>>;

// -------------------------------------------------------------------- hooks
export type NotifOptions = Assert<
  Mutual<Impl.UseNotificationsOptions, Spec.UseNotificationsOptions>
>;
export type NotifResult = Assert<Mutual<Impl.UseNotificationsResult, Spec.UseNotificationsResult>>;
export type NotifResultTyped = Assert<
  Mutual<
    Impl.UseNotificationsResult<{ amount: number }>,
    Spec.UseNotificationsResult<{ amount: number }>
  >
>;
export type UseNotifications = Assert<
  Extends<typeof Impl.useNotifications, typeof Spec.useNotifications>
>;
export type CountResult = Assert<Mutual<Impl.UseCountResult, Spec.UseCountResult>>;
export type UseUnread = Assert<Extends<typeof Impl.useUnreadCount, typeof Spec.useUnreadCount>>;
export type UseUnseen = Assert<Extends<typeof Impl.useUnseenCount, typeof Spec.useUnseenCount>>;
export type PrefsResult = Assert<Mutual<Impl.UsePreferencesResult, Spec.UsePreferencesResult>>;
export type UsePrefs = Assert<Extends<typeof Impl.usePreferences, typeof Spec.usePreferences>>;

// ------------------------------------------------------------------- inbox
export type Slot = Assert<Mutual<Impl.InboxSlot, Spec.InboxSlot>>;
export type Appearance = Assert<Mutual<Impl.InboxAppearance, Spec.InboxAppearance>>;
export type Localization = Assert<Mutual<Impl.InboxLocalization, Spec.InboxLocalization>>;
export type Props = Assert<Mutual<Impl.InboxProps, Spec.InboxProps>>;
export type PropsTyped = Assert<
  Mutual<Impl.InboxProps<{ amount: number }>, Spec.InboxProps<{ amount: number }>>
>;
export type InboxComponent = Assert<Extends<typeof Impl.Inbox, typeof Spec.Inbox>>;

// The spec's client types as seen through the react module must stay
// structurally interchangeable with the client package's own (sanity check
// of the rename pipeline, not of the implementation).
export type CrossModuleItem = Assert<Mutual<SpecClient.InboxItem, ImplClient.InboxItem>>;
