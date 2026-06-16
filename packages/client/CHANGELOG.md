# @dronte/client

## 0.1.0

### Minor Changes

- 14b92d5: Generated API types now cover the Phase 3 server surface: the notification
  status timeline endpoint (`GET /v1/notifications/{id}/timeline`, statuses
  `created`/`delivered_hint`/`seen`/`read`) and the 429 rate-limit responses
  (with `Retry-After`) the server now enforces on creates and the inbox list.
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

- 48443a1: Regenerate client types for the admin multi-user auth surface: login,
  logout, current-user, and user CRUD endpoints with their role and session
  schemas.
- 01464ce: Generated API types now declare the `400` validation-error response
  (`application/json` `Error`) on the inbox list (`GET /v1/inbox/items`) and
  upsert-subscriber (`PUT /v1/subscribers/{subscriber_id}`) operations, matching
  the responses the server already returns.
