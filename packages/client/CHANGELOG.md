# @chimely/client

## 0.2.0

### Minor Changes

- 4488eb4: Archive round trip: `archive`/`unarchive` per item with optimistic removal from the active view, `archiveAll` (watermark move server-side), and `archiveRead` (asynchronous; the snapshot converges via the completion hint). `InboxItem` gains `archived` and `setFilter` accepts `'archived'`.
- 4488eb4: Read-state round trip and server-side views: `markUnread(item)` flips an item back to unread with the same optimistic/rollback semantics as `markRead`, and `setFilter('default' | 'unread')` switches the server-side list view, resetting pagination and refetching. The snapshot gains `filter`.
- 4488eb4: `InboxSnapshot` gains `lastRefreshNewItemIds`: the ids the last first-page merge added that were not already loaded, with fresh array identity per merge. 304 refreshes and fetchMore leave it untouched. Powers the new-notification indicator in @chimely/react.

## 0.1.0

### Minor Changes

- Initial public release.

### Patch Changes

- bb1f022: Comment and JSDoc cleanup. No API, type, or behavior change.
- 12e5a50: Regenerate client types from the documented error contract: `Error.code` is now
  a typed enum of the stable error codes, and the inbox stream declares its 429
  (`too_many_connections`) response. Repository URLs point at dodopayments/chimely.
