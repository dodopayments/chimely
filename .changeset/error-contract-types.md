---
'@chimely/client': patch
'@chimely/react': patch
'docs': patch
---

Regenerate client types from the documented error contract: `Error.code` is now
a typed enum of the stable error codes, and the inbox stream declares its 429
(`too_many_connections`) response. Repository URLs point at dodopayments/chimely.
