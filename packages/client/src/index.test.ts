import { expect, test } from 'vitest';
import { BACKOFF_DEFAULTS, ChimelyClient, ChimelyError } from './index';

test('public surface exports', () => {
  expect(typeof ChimelyClient).toBe('function');
  expect(typeof ChimelyError).toBe('function');
  expect(BACKOFF_DEFAULTS.initialDelayMs).toBe(1000);
});

test('ChimelyError carries code and status', () => {
  const error = new ChimelyError('nope', { code: 'unauthorized', status: 401 });
  expect(error).toBeInstanceOf(Error);
  expect(error.code).toBe('unauthorized');
  expect(error.status).toBe(401);
  expect(error.name).toBe('ChimelyError');
});
