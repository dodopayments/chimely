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

  test('re-injects after head-replacing navigation removes the tag', () => {
    ensureStyles();
    expect(document.querySelector('style[data-chimely]')).not.toBeNull();

    // Turbo and PJAX swap <head> wholesale while the module instance and
    // its state survive.
    document.querySelector('style[data-chimely]')?.remove();
    ensureStyles();
    expect(document.querySelector('style[data-chimely]')).not.toBeNull();
  });

  test('never injects a second tag', () => {
    ensureStyles();
    ensureStyles();
    expect(document.querySelectorAll('style[data-chimely]')).toHaveLength(1);
  });
});
