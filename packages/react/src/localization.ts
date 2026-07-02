import { formatRelativeTime } from './time';

/**
 * No index signature so typos fail to type-check.
 * New fields are added as optional to keep minor versions non-breaking.
 * DEFAULT_LOCALIZATION supplies a value for every field, so merged results
 * are fully populated.
 */
export interface InboxLocalization {
  emptyTitle: string;
  emptyBody: string;
  markAllRead: string;
  preferencesTitle: string;
  /** List-view header title. Also labels the popover dialog. */
  inboxTitle?: string;
  /** Bell button aria-label. */
  bellLabel?: string;
  /** Preferences back button aria-label. */
  backLabel?: string;
  /** New-notification indicator text. */
  newNotifications?: (count: number) => string;
  /**
   * Display names for category keys in the preferences panel.
   * Merged by whole-map replacement, not per key.
   */
  categoryLabels?: Record<string, string>;
  /** Timestamp text in the default item rendering. Defaults to relative time. */
  formatTimestamp?: (iso: string) => string;
}

export const DEFAULT_LOCALIZATION: Required<InboxLocalization> = {
  emptyTitle: 'No notifications',
  emptyBody: "You're all caught up.",
  markAllRead: 'Mark all as read',
  preferencesTitle: 'Notification preferences',
  inboxTitle: 'Notifications',
  bellLabel: 'Notifications',
  backLabel: 'Back',
  newNotifications: (count) => (count === 1 ? '1 new notification' : `${count} new notifications`),
  categoryLabels: {},
  formatTimestamp: (iso) => formatRelativeTime(iso),
};

export function mergeLocalization(
  overrides?: Partial<InboxLocalization>,
): Required<InboxLocalization> {
  const merged = { ...DEFAULT_LOCALIZATION };
  if (!overrides) {
    return merged;
  }
  const apply = <K extends keyof InboxLocalization>(key: K): void => {
    const value = overrides[key];
    if (value !== undefined) {
      // The undefined check narrows the value, which the indexed write cannot see.
      merged[key] = value as Required<InboxLocalization>[K];
    }
  };
  for (const key of Object.keys(DEFAULT_LOCALIZATION) as Array<keyof InboxLocalization>) {
    apply(key);
  }
  return merged;
}
