import { expect, test } from 'vitest';
import { BACKOFF_DEFAULTS, DronteClient, DronteError } from './index';

test('public surface exports', () => {
  expect(typeof DronteClient).toBe('function');
  expect(typeof DronteError).toBe('function');
  expect(BACKOFF_DEFAULTS.initialDelayMs).toBe(1000);
});

test('DronteError carries code and status', () => {
  const error = new DronteError('nope', { code: 'unauthorized', status: 401 });
  expect(error).toBeInstanceOf(Error);
  expect(error.code).toBe('unauthorized');
  expect(error.status).toBe(401);
  expect(error.name).toBe('DronteError');
});
