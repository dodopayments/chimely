/**
 * Indirection over window navigation so item-click navigation is spyable in
 * tests. jsdom's window.location properties are not configurable, so
 * location.assign cannot be spied directly.
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
 * Following a `javascript:`, `data:`, or custom-scheme URI would execute in
 * the embedding page's origin. Only targets that resolve to http(s) may
 * navigate. Relative URLs resolve against the embedding page and stay
 * same-origin, so they pass.
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
