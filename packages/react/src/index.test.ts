import { expect, test } from 'vitest';
import type { InboxTab } from './index';
import {
  Bell,
  ChimelyProvider,
  DEFAULT_LOCALIZATION,
  INBOX_CSS,
  Inbox,
  InboxContent,
  Preferences,
  useChimelyClient,
  useInbox,
  useNotifications,
  usePreferences,
  useUnreadCount,
  useUnseenCount,
} from './index';

interface TrackedTab extends InboxTab {
  analyticsId: string;
}

test('public surface exports', () => {
  expect(typeof ChimelyProvider).toBe('function');
  expect(typeof useChimelyClient).toBe('function');
  expect(typeof useNotifications).toBe('function');
  expect(typeof useUnreadCount).toBe('function');
  expect(typeof useUnseenCount).toBe('function');
  expect(typeof usePreferences).toBe('function');
  expect(typeof useInbox).toBe('function');
  expect(typeof Inbox).toBe('function');
  expect(typeof InboxContent).toBe('function');
  expect(typeof Preferences).toBe('function');
  // forwardRef components are objects with a render function.
  expect(Bell).toBeDefined();
  expect(DEFAULT_LOCALIZATION.markAllRead.length).toBeGreaterThan(0);
  expect(INBOX_CSS).toContain('.chimely-popover');
});

test('InboxTab remains an extendable public interface', () => {
  const tab: TrackedTab = {
    label: 'Billing',
    categories: ['billing.alerts'],
    analyticsId: 'billing',
  };
  expect(tab.analyticsId).toBe('billing');
});
