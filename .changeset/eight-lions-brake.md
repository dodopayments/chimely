---
"@chimely/client": minor
---

Archive round trip: `archive`/`unarchive` per item with optimistic removal from the active view, `archiveAll` (watermark move server-side), and `archiveRead` (asynchronous; the snapshot converges via the completion hint). `InboxItem` gains `archived` and `setFilter` accepts `'archived'`.
