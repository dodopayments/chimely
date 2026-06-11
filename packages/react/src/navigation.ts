/**
 * Indirection over window navigation so the default item-click behavior
 * (follow payload.action_url after mark-read) is observable in tests.
 * jsdom's window.location properties are not configurable, which rules out
 * spying on location.assign directly.
 */
export const navigation = {
  assign(url: string): void {
    if (typeof window !== 'undefined') {
      window.location.assign(url);
    }
  },
};

/**
 * action_url is customer-supplied data stored verbatim by the server.
 * Following it blindly lets a `javascript:` (or `data:`, or custom-scheme)
 * URI execute in the embedding page's origin on item click. Only targets
 * that resolve to http(s) may navigate. Relative URLs resolve against the
 * embedding page and stay same-origin, so they pass.
 */
export function isSafeActionUrl(url: string): boolean {
  if (typeof window === 'undefined') {
    return false;
  }
  try {
    const target = new URL(url, window.location.href);
    return target.protocol === 'https:' || target.protocol === 'http:';
  } catch {
    return false;
  }
}
