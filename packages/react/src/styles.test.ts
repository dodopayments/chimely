import { afterEach, describe, expect, test } from 'vitest';
import { ensureStyles } from './styles';

afterEach(() => {
  document.querySelector('style[data-chimely]')?.remove();
});

describe('ensureStyles', () => {
  test('the popover width is clamped to the viewport', () => {
    ensureStyles();
    const style = document.querySelector('style[data-chimely]');
    expect(style?.textContent).toContain('width: min(360px, calc(100vw - 16px));');
  });
});
