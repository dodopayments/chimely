---
"@chimely/client": minor
---

Read-state round trip and server-side views: `markUnread(item)` flips an item back to unread with the same optimistic/rollback semantics as `markRead`, and `setFilter('default' | 'unread')` switches the server-side list view, resetting pagination and refetching. The snapshot gains `filter`.
