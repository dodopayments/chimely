/**
 * Hooks and <Inbox /> on top of @chimely/client.
 *
 * The public surface is stable and additive-only.
 * Zero styling dependencies. @floating-ui/dom is the only runtime UI dependency.
 */

export type { ChimelyProviderProps } from './context';
export { ChimelyProvider, useChimelyClient } from './context';
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
