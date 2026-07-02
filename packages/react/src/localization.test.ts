import { describe, expect, test } from 'vitest';
import type { InboxLocalization } from './localization';
import { DEFAULT_LOCALIZATION, mergeLocalization } from './localization';

describe('mergeLocalization', () => {
  test('no overrides yields a copy of the defaults', () => {
    const merged = mergeLocalization();
    expect(merged).toEqual(DEFAULT_LOCALIZATION);
    expect(merged).not.toBe(DEFAULT_LOCALIZATION);
  });

  test('string overrides win, unspecified keys keep defaults', () => {
    const merged = mergeLocalization({ inboxTitle: 'Meldungen' });
    expect(merged.inboxTitle).toBe('Meldungen');
    expect(merged.markAllRead).toBe(DEFAULT_LOCALIZATION.markAllRead);
    expect(merged.bellLabel).toBe('Notifications');
  });

  test('function overrides win', () => {
    const formatTimestamp = (iso: string) => `ts:${iso}`;
    const newNotifications = (count: number) => `${count}!`;
    const merged = mergeLocalization({ formatTimestamp, newNotifications });
    expect(merged.formatTimestamp('x')).toBe('ts:x');
    expect(merged.newNotifications(3)).toBe('3!');
  });

  test('categoryLabels replaces the whole map', () => {
    const merged = mergeLocalization({ categoryLabels: { 'billing.alerts': 'Billing' } });
    expect(merged.categoryLabels).toEqual({ 'billing.alerts': 'Billing' });
  });

  test('explicit undefined values keep defaults', () => {
    const merged = mergeLocalization({ emptyTitle: undefined });
    expect(merged.emptyTitle).toBe(DEFAULT_LOCALIZATION.emptyTitle);
  });

  test('keys outside the contract are dropped', () => {
    const merged = mergeLocalization({ bogus: 'x' } as Partial<InboxLocalization>);
    expect('bogus' in merged).toBe(false);
  });

  test('default newNotifications pluralizes', () => {
    expect(DEFAULT_LOCALIZATION.newNotifications(1)).toBe('1 new notification');
    expect(DEFAULT_LOCALIZATION.newNotifications(4)).toBe('4 new notifications');
  });
});
