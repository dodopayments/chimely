# @dronte/react

## 0.1.0

### Minor Changes

- ca89a02: Default `<Inbox />` theme now ships the Dronte brand palette: primary actions,
  links, the unread badge, the unread dot, and focus rings use the accent
  `#1264FF`; hover/section accents use the deep accent `#004F32`. Adds a
  `colorPrimaryHover` appearance variable and accent focus-visible outlines.
  All values remain overridable through the existing `--dronte-*` custom
  properties — no new styling dependency.
- 30a6b06: First release of the Dronte SDKs, implementing the frozen v1 surface
  (specs/sdk-api.d.ts, contract-v1).

  `@dronte/client`: the framework-agnostic headless inbox — SSE hint
  consumption with jittered-backoff reconnect and resume, ETag-conditional
  refetch, optimistic read state with rollback, merged-stream keyset
  pagination, HMAC subscriber auth forwarding.

  `@dronte/react`: `DronteProvider`, `useNotifications`, `useUnreadCount`,
  `useUnseenCount`, `usePreferences`, `useInbox`, and the drop-in `<Inbox />`
  (bell, badge, popover, infinite scroll, preference panel, render-prop
  overrides, plain-CSS custom-property theming).

### Patch Changes

- Updated dependencies [48443a1]
- Updated dependencies [01464ce]
- Updated dependencies [14b92d5]
- Updated dependencies [30a6b06]
  - @dronte/client@0.1.0
