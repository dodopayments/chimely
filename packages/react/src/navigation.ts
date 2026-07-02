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

export type ResolvedActionUrl =
  | { kind: 'same-origin'; path: string }
  | { kind: 'external'; href: string };

/**
 * Classifies a safe action_url for navigation. Same-origin targets are
 * normalized to the path form SPA routers expect. Unsafe targets (per
 * isSafeActionUrl) return null and must not navigate.
 */
export function resolveActionUrl(url: string): ResolvedActionUrl | null {
  if (!isSafeActionUrl(url)) {
    return null;
  }
  const target = new URL(url, window.location.href);
  if (target.origin === window.location.origin) {
    return { kind: 'same-origin', path: `${target.pathname}${target.search}${target.hash}` };
  }
  return { kind: 'external', href: url };
}
