/**
 * No index signature so typos fail to type-check.
 * New strings are added as optional fields to keep minor versions non-breaking.
 */
export interface InboxLocalization {
  emptyTitle: string;
  emptyBody: string;
  markAllRead: string;
  preferencesTitle: string;
}

export const DEFAULT_LOCALIZATION: InboxLocalization = {
  emptyTitle: 'No notifications',
  emptyBody: "You're all caught up.",
  markAllRead: 'Mark all as read',
  preferencesTitle: 'Notification preferences',
};

export function mergeLocalization(overrides?: Partial<InboxLocalization>): InboxLocalization {
  const merged = { ...DEFAULT_LOCALIZATION };
  if (!overrides) {
    return merged;
  }
  for (const key of Object.keys(DEFAULT_LOCALIZATION) as Array<keyof InboxLocalization>) {
    const value = overrides[key];
    if (value !== undefined) {
      merged[key] = value;
    }
  }
  return merged;
}
