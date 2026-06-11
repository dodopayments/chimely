---
"@dronte/client": minor
---

Generated API types now cover the Phase 3 server surface: the notification
status timeline endpoint (`GET /v1/notifications/{id}/timeline`, statuses
`created`/`delivered_hint`/`seen`/`read`) and the 429 rate-limit responses
(with `Retry-After`) the server now enforces on creates and the inbox list.
