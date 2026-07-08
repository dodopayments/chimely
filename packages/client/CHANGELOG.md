# @chimely/client

## 0.2.1

### Patch Changes

- Ship the package README on npm.

## 0.2.0

### Minor Changes

- Read state and views: `markUnread(item)` flips an item back to unread with the same optimistic and rollback semantics as `markRead`, and `setFilter('default' | 'unread' | 'archived')` switches the server-side list view, resetting pagination and refetching. The snapshot gains `filter`.
- Archive: `archive` and `unarchive` per item with optimistic removal from the active view, `archiveAll`, and `archiveRead` (asynchronous, the snapshot converges on the completion hint). `InboxItem` gains `archived`.
- `InboxSnapshot.lastRefreshNewItemIds` lists the ids the last refresh merged in. 304 refreshes and `fetchMore` leave it untouched. Powers the new-notification pill in `@chimely/react`.

## 0.1.0

### Minor Changes

- Initial public release.

### Patch Changes

- Comment and JSDoc cleanup. No API, type, or behavior change.
- Regenerate client types from the documented error contract: `Error.code` is now
  a typed enum of the stable error codes, and the inbox stream declares its 429
  (`too_many_connections`) response. Repository URLs point at dodopayments/chimely.
