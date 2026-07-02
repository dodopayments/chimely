import { describe, expect, test } from 'vitest';
import { resolveActionUrl } from './navigation';

describe('resolveActionUrl', () => {
  test('relative URLs resolve same-origin in path form', () => {
    expect(resolveActionUrl('/invoices/42?x=1#y')).toEqual({
      kind: 'same-origin',
      path: '/invoices/42?x=1#y',
    });
  });

  test('absolute same-origin URLs are normalized to the path form', () => {
    expect(resolveActionUrl(`${window.location.origin}/invoices/9?q=1#top`)).toEqual({
      kind: 'same-origin',
      path: '/invoices/9?q=1#top',
    });
  });

  test('cross-origin URLs stay external with the original href', () => {
    expect(resolveActionUrl('https://elsewhere.test/x')).toEqual({
      kind: 'external',
      href: 'https://elsewhere.test/x',
    });
  });

  test('unsafe protocols return null', () => {
    expect(resolveActionUrl('javascript:alert(1)')).toBeNull();
    expect(resolveActionUrl('data:text/html,x')).toBeNull();
  });
});
