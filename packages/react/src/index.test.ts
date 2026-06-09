import { expect, test } from 'vitest';
import { CORE, SDK_NAME } from './index.js';

test('package identity', () => {
  expect(SDK_NAME).toBe('@dronte/react');
  expect(CORE).toBe('@dronte/client');
});
