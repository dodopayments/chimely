# @chimely/react

## 0.2.0

### Minor Changes

- 4488eb4: Popover robustness: opt-in `portal` rendering to `document.body` with fixed positioning (escapes overflow and transform ancestors), `placementOffset`, controlled `open`/`onOpenChange`, and `routerPush` for SPA navigation on same-origin action URLs. `react-dom` is now a peer dependency. markAllSeen fires on every open transition, including controlled and programmatic opens.
- 4488eb4: Read/unread toggle on row hover (tooltips via `localization.markReadAction`/`markUnreadAction`) and a header view select (Inbox / Unread, strings via `filterLabel`/`filterInbox`/`filterUnread`) wired to the server-side unread filter. New `filter` appearance slot.
- 4488eb4: Archive UI: row hover gains an archive/unarchive toggle, the view select gains Archived, and the header grows a more-actions menu (mark all read, archive read, archive all). New localization keys: `filterArchived`, `archiveAction`, `unarchiveAction`, `moreActions`, `archiveAllAction`, `archiveReadAction`. Mark-all-read moves from a bare header button into the menu.
- 4488eb4: Appearance depth and the new-notification pill: exported `darkTheme` variables preset, per-slot inline `appearance.styles`, `appearance.icons` overrides for the bell and gear, new `colorBadgeForeground` and `shadow` variables. Arrivals while the list is scrolled down surface as a "N new notifications" pill (text via `localization.newNotifications`) instead of yanking the viewport. The list gains a `.chimely-list-container` wrapper.
- 4488eb4: Composable components: `<InboxContent />` (the popover interior for custom popovers, drawers, and full-page inboxes), `<Bell />` (forwardRef trigger with the unseen badge), and `<Preferences />` are now exported. Granular render props `renderSubject`, `renderBody`, and `renderAvatar` replace one fragment of the default item while keeping layout and click wiring. The popover interior is now wrapped in a `.chimely-content` div (new `content` slot).
- 4488eb4: Tabs: `tabs={[{ label, icon?, filter? }]}` on `<Inbox />` and `<InboxContent />` renders a tab strip with client-side predicate filtering and per-tab unread counts over loaded pages. When a sparse tab's filtered view runs short, the list keeps fetching until the tab fills or pages run out. Infinite scroll now drains continuously while the end sentinel stays visible, instead of one page per intersection event.
- 4488eb4: Inbox popover polish: `renderFooter` render prop, a list-view header title, relative timestamps in the default item (overridable via `localization.formatTimestamp`), and a localization pass covering the bell and back-button aria-labels plus `categoryLabels` display names in the preferences panel.

### Patch Changes

- Updated dependencies [4488eb4]
- Updated dependencies [4488eb4]
- Updated dependencies [4488eb4]
  - @chimely/client@0.2.0

## 0.1.0

### Minor Changes

- Initial public release.

### Patch Changes

- bb1f022: Comment and JSDoc cleanup. No API, type, or behavior change.
- 12e5a50: Regenerate client types from the documented error contract: `Error.code` is now
  a typed enum of the stable error codes, and the inbox stream declares its 429
  (`too_many_connections`) response. Repository URLs point at dodopayments/chimely.
- Updated dependencies [bb1f022]
- Updated dependencies [12e5a50]
- Updated dependencies
  - @chimely/client@0.1.0
