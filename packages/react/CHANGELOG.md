# @chimely/react

## 0.2.0

### Minor Changes

- Composable components: `<InboxContent />`, `<Bell />`, and `<Preferences />` are exported for custom popovers, drawers, and full-page inboxes. New render props `renderSubject`, `renderBody`, and `renderAvatar` replace one fragment of the default item while keeping its layout and click wiring.
- Tabs: `tabs={[{ label, icon?, filter? }]}` renders a tab strip with client-side filtering, per-tab unread counts, and full keyboard support. Infinite scroll now keeps fetching while the end sentinel stays visible, so sparse tabs fill themselves.
- Popover: opt-in `portal` rendering to `document.body` (escapes overflow and transform ancestors), `placementOffset`, controlled `open`/`onOpenChange`, and `routerPush` for SPA navigation on same-origin action URLs. `react-dom` is now a peer dependency.
- Theming: exported `darkTheme` preset, per-slot inline `appearance.styles`, `appearance.icons` overrides for the bell and gear, and new `colorBadgeForeground` and `shadow` variables.
- New-notification pill: arrivals while the list is scrolled down surface as a pill instead of moving the viewport. Text via `localization.newNotifications`.
- Read state: a read/unread toggle on row hover and a header view select (Inbox / Unread) backed by the server-side filter.
- Archive: an archive/unarchive toggle on row hover, an Archived view, and a header more-actions menu with mark all read, archive read, and archive all. Mark-all-read moved from a bare header button into this menu.
- Polish: `renderFooter`, a header title, relative timestamps by default (override with `localization.formatTimestamp`), and localization keys for every new string.

### Patch Changes

- Updated dependencies:
  - @chimely/client@0.2.0

## 0.1.0

### Minor Changes

- Initial public release.

### Patch Changes

- Comment and JSDoc cleanup. No API, type, or behavior change.
- Regenerate client types from the documented error contract: `Error.code` is now
  a typed enum of the stable error codes, and the inbox stream declares its 429
  (`too_many_connections`) response. Repository URLs point at dodopayments/chimely.
- Updated dependencies:
  - @chimely/client@0.1.0
