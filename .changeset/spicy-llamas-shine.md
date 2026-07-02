---
"@chimely/react": minor
---

Tabs: `tabs={[{ label, icon?, filter? }]}` on `<Inbox />` and `<InboxContent />` renders a tab strip with client-side predicate filtering and per-tab unread counts over loaded pages. When a sparse tab's filtered view runs short, the list keeps fetching until the tab fills or pages run out. Infinite scroll now drains continuously while the end sentinel stays visible, instead of one page per intersection event.
