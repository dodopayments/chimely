import { describe, expect, test } from 'vitest';
import { formatRelativeTime } from './time';

const NOW = Date.parse('2026-07-02T12:00:00Z');

function at(offsetSeconds: number): string {
  return new Date(NOW + offsetSeconds * 1000).toISOString();
}

function relative(value: number, unit: Intl.RelativeTimeFormatUnit): string {
  return new Intl.RelativeTimeFormat(undefined, { numeric: 'auto' }).format(value, unit);
}

describe('formatRelativeTime', () => {
  test('under a minute reads as now, in both directions', () => {
    expect(formatRelativeTime(at(-30), NOW)).toBe('now');
    expect(formatRelativeTime(at(30), NOW)).toBe('now');
    expect(formatRelativeTime(at(-59), NOW)).toBe('now');
  });

  test('minutes', () => {
    expect(formatRelativeTime(at(-60), NOW)).toBe(relative(-1, 'minute'));
    expect(formatRelativeTime(at(-5 * 60), NOW)).toBe(relative(-5, 'minute'));
    expect(formatRelativeTime(at(5 * 60), NOW)).toBe(relative(5, 'minute'));
  });

  test('hours', () => {
    expect(formatRelativeTime(at(-3 * 3600), NOW)).toBe(relative(-3, 'hour'));
    expect(formatRelativeTime(at(-90 * 60), NOW)).toBe(relative(-1, 'hour'));
  });

  test('days up to a week', () => {
    expect(formatRelativeTime(at(-2 * 86400), NOW)).toBe(relative(-2, 'day'));
    expect(formatRelativeTime(at(-6 * 86400), NOW)).toBe(relative(-6, 'day'));
  });

  test('past a week falls back to the locale date', () => {
    const iso = at(-10 * 86400);
    expect(formatRelativeTime(iso, NOW)).toBe(new Date(iso).toLocaleDateString());
  });

  test('invalid input is returned verbatim', () => {
    expect(formatRelativeTime('not-a-date', NOW)).toBe('not-a-date');
  });
});
