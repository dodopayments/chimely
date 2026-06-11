import { cleanup } from '@testing-library/react';
import { afterEach } from 'vitest';

// React act() integration for a non-jest runner.
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

// RTL auto-cleanup needs vitest globals. They are off, so clean up explicitly.
afterEach(() => {
  cleanup();
});
