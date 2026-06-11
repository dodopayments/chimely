---
'@dronte/client': minor
'@dronte/react': minor
---

First release of the Dronte SDKs, implementing the frozen v1 surface
(specs/sdk-api.d.ts, contract-v1).

`@dronte/client`: the framework-agnostic headless inbox — SSE hint
consumption with jittered-backoff reconnect and resume, ETag-conditional
refetch, optimistic read state with rollback, merged-stream keyset
pagination, HMAC subscriber auth forwarding.

`@dronte/react`: `DronteProvider`, `useNotifications`, `useUnreadCount`,
`useUnseenCount`, `usePreferences`, `useInbox`, and the drop-in `<Inbox />`
(bell, badge, popover, infinite scroll, preference panel, render-prop
overrides, plain-CSS custom-property theming).
