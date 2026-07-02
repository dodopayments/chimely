import type { InboxSnapshot } from './types';

/**
 * Every patch produces a new snapshot object so `useSyncExternalStore`
 * consumers can compare by identity.
 */
export class InboxStore<TPayload> {
  private snapshot: InboxSnapshot<TPayload>;
  private readonly listeners = new Set<() => void>();

  constructor() {
    this.snapshot = {
      items: [],
      counts: { unread: 0, unseen: 0 },
      status: 'idle',
      hasMore: true,
      isLoading: false,
      error: null,
      lastRefreshNewItemIds: [],
      filter: 'default',
    };
  }

  getSnapshot(): InboxSnapshot<TPayload> {
    return this.snapshot;
  }

  subscribe(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }

  patch(patch: Partial<InboxSnapshot<TPayload>>): void {
    this.snapshot = { ...this.snapshot, ...patch };
    for (const listener of [...this.listeners]) {
      listener();
    }
  }
}
