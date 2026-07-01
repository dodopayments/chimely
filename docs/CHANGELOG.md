# docs

## 0.0.1

### Patch Changes

- bb1f022: Comment and JSDoc cleanup. No API, type, or behavior change.
- 12e5a50: Regenerate client types from the documented error contract: `Error.code` is now
  a typed enum of the stable error codes, and the inbox stream declares its 429
  (`too_many_connections`) response. Repository URLs point at dodopayments/chimely.
