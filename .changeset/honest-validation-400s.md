---
"@dronte/client": patch
---

Generated API types now declare the `400` validation-error response
(`application/json` `Error`) on the inbox list (`GET /v1/inbox/items`) and
upsert-subscriber (`PUT /v1/subscribers/{subscriber_id}`) operations, matching
the responses the server already returns.
