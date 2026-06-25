import { expect, test } from 'vitest';
import {
  ChimelyProvider,
  DEFAULT_LOCALIZATION,
  Inbox,
  useChimelyClient,
  useInbox,
  useNotifications,
  usePreferences,
  useUnreadCount,
  useUnseenCount,
} from './index';

test('public surface exports', () => {
  expect(typeof ChimelyProvider).toBe('function');
  expect(typeof useChimelyClient).toBe('function');
  expect(typeof useNotifications).toBe('function');
  expect(typeof useUnreadCount).toBe('function');
  expect(typeof useUnseenCount).toBe('function');
  expect(typeof usePreferences).toBe('function');
  expect(typeof useInbox).toBe('function');
  expect(typeof Inbox).toBe('function');
  expect(DEFAULT_LOCALIZATION.markAllRead.length).toBeGreaterThan(0);
});
