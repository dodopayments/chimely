import { expect, test } from 'vitest';
import { SDK_NAME } from './index.js';

test('package identity', () => {
  expect(SDK_NAME).toBe('@dronte/client');
});
