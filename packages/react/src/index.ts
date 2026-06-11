/**
 * @dronte/react, the hooks and <Inbox /> on top of @dronte/client.
 *
 * The public surface is frozen in specs/sdk-api.d.ts (additive-only from
 * the contract-v1 tag). Zero styling dependencies: plain CSS custom
 * properties, slot classNames, render props. @floating-ui/dom is the only
 * runtime UI dependency.
 */

export type { DronteProviderProps } from './context';
export { DronteProvider, useDronteClient } from './context';
export type {
  UseCountResult,
  UseInboxResult,
  UseNotificationsOptions,
  UseNotificationsResult,
  UsePreferencesResult,
} from './hooks';
export {
  useInbox,
  useNotifications,
  usePreferences,
  useUnreadCount,
  useUnseenCount,
} from './hooks';
export type { InboxAppearance, InboxProps, InboxSlot } from './Inbox';
export { Inbox } from './Inbox';
export type { InboxLocalization } from './localization';
export { DEFAULT_LOCALIZATION } from './localization';
