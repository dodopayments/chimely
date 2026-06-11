import { describe, expect, test } from 'vitest';
import { BACKOFF_DEFAULTS, backoffDelayMs, resolveBackoff } from './backoff';

describe('resolveBackoff', () => {
  test('documented defaults', () => {
    expect(resolveBackoff()).toEqual({
      initialDelayMs: 1000,
      maxDelayMs: 30000,
      multiplier: 2,
      jitter: 0.5,
      maxAttempts: Number.POSITIVE_INFINITY,
    });
    expect(resolveBackoff()).toEqual(BACKOFF_DEFAULTS);
  });

  test('partial config keeps remaining defaults', () => {
    const resolved = resolveBackoff({ initialDelayMs: 50, maxAttempts: 3 });
    expect(resolved.initialDelayMs).toBe(50);
    expect(resolved.maxAttempts).toBe(3);
    expect(resolved.maxDelayMs).toBe(30000);
    expect(resolved.multiplier).toBe(2);
    expect(resolved.jitter).toBe(0.5);
  });
});

describe('backoffDelayMs', () => {
  const noJitter = resolveBackoff({ jitter: 0 });

  test('exponential growth from initialDelayMs', () => {
    expect(backoffDelayMs(1, noJitter)).toBe(1000);
    expect(backoffDelayMs(2, noJitter)).toBe(2000);
    expect(backoffDelayMs(3, noJitter)).toBe(4000);
    expect(backoffDelayMs(4, noJitter)).toBe(8000);
  });

  test('caps at maxDelayMs', () => {
    expect(backoffDelayMs(6, noJitter)).toBe(30000);
    expect(backoffDelayMs(50, noJitter)).toBe(30000);
  });

  test('jitter spreads around the base delay', () => {
    const resolved = resolveBackoff();
    expect(backoffDelayMs(3, resolved, () => 0)).toBe(2000);
    expect(backoffDelayMs(3, resolved, () => 0.5)).toBe(4000);
    expect(backoffDelayMs(3, resolved, () => 1)).toBe(6000);
  });

  test('many simulated drops do not reconnect in lockstep', () => {
    // Deploy-time thundering-herd protection. A fleet of clients dropped at
    // once must spread over the jitter window instead of sharing one delay.
    const resolved = resolveBackoff();
    const fleet = Array.from({ length: 200 }, () => backoffDelayMs(3, resolved));
    const base = 4000;
    for (const delay of fleet) {
      expect(delay).toBeGreaterThanOrEqual(base * 0.5);
      expect(delay).toBeLessThanOrEqual(base * 1.5);
    }
    const distinct = new Set(fleet);
    expect(distinct.size).toBeGreaterThan(50);
    const mean = fleet.reduce((sum, delay) => sum + delay, 0) / fleet.length;
    const variance = fleet.reduce((sum, delay) => sum + (delay - mean) ** 2, 0) / fleet.length;
    expect(Math.sqrt(variance)).toBeGreaterThan(100);
  });
});
