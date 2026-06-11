import type { BackoffConfig } from './types';

export interface ResolvedBackoff {
  initialDelayMs: number;
  maxDelayMs: number;
  multiplier: number;
  jitter: number;
  maxAttempts: number;
}

/** The documented defaults from specs/sdk-api.d.ts. */
export const BACKOFF_DEFAULTS: ResolvedBackoff = {
  initialDelayMs: 1000,
  maxDelayMs: 30000,
  multiplier: 2,
  jitter: 0.5,
  maxAttempts: Number.POSITIVE_INFINITY,
};

export function resolveBackoff(config?: BackoffConfig): ResolvedBackoff {
  return {
    initialDelayMs: config?.initialDelayMs ?? BACKOFF_DEFAULTS.initialDelayMs,
    maxDelayMs: config?.maxDelayMs ?? BACKOFF_DEFAULTS.maxDelayMs,
    multiplier: config?.multiplier ?? BACKOFF_DEFAULTS.multiplier,
    jitter: config?.jitter ?? BACKOFF_DEFAULTS.jitter,
    maxAttempts: config?.maxAttempts ?? BACKOFF_DEFAULTS.maxAttempts,
  };
}

/**
 * Delay before reconnect attempt `attempt` (1-based consecutive failure
 * count). The cap applies to the base delay, then jitter spreads the result
 * over ±(jitter × base) so dropped clients do not reconnect in lockstep.
 */
export function backoffDelayMs(
  attempt: number,
  backoff: ResolvedBackoff,
  random: () => number = Math.random,
): number {
  const exponent = Math.max(0, attempt - 1);
  const base = Math.min(
    backoff.maxDelayMs,
    backoff.initialDelayMs * backoff.multiplier ** exponent,
  );
  const jittered = base * (1 + backoff.jitter * (2 * random() - 1));
  return Math.max(0, Math.round(jittered));
}
