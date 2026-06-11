import { expect, test } from 'vitest';
import {
  DEFAULT_LOCALIZATION,
  DronteProvider,
  Inbox,
  useDronteClient,
  useInbox,
  useNotifications,
  usePreferences,
  useUnreadCount,
  useUnseenCount,
} from './index';

test('public surface exports', () => {
  expect(typeof DronteProvider).toBe('function');
  expect(typeof useDronteClient).toBe('function');
  expect(typeof useNotifications).toBe('function');
  expect(typeof useUnreadCount).toBe('function');
  expect(typeof useUnseenCount).toBe('function');
  expect(typeof usePreferences).toBe('function');
  expect(typeof useInbox).toBe('function');
  expect(typeof Inbox).toBe('function');
  expect(DEFAULT_LOCALIZATION.markAllRead.length).toBeGreaterThan(0);
});
