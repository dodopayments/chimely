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
