/**
 * Hooks and <Inbox /> on top of @chimely/client.
 *
 * The public surface is stable and additive-only.
 * Zero styling dependencies. @floating-ui/dom is the only runtime UI dependency.
 *
 * <Inbox /> is the drop-in component. Bell, InboxContent, and Preferences
 * compose custom popovers, drawers, and full-page inboxes.
 */

export type { InboxAppearance, InboxSlot } from './appearance';
export { darkTheme } from './appearance';
export type { BellProps } from './components/Bell';
export { Bell } from './components/Bell';
export type { InboxContentProps, InboxTab } from './components/InboxContent';
export { InboxContent } from './components/InboxContent';
export type { PreferencesProps } from './components/Preferences';
export { Preferences } from './components/Preferences';
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
export type { InboxProps } from './Inbox';
export { Inbox } from './Inbox';
export type { InboxLocalization } from './localization';
export { DEFAULT_LOCALIZATION } from './localization';
