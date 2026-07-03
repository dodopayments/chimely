---
'@chimely/react': patch
---

Hostile-host hardening: the bundle now carries its own `'use client'`
directive (imports cleanly from React Server Component module graphs), the
bell's accessible name includes the unseen count (previously invisible to
assistive tech), and `INBOX_CSS` is exported so nonce-CSP hosts can serve the
stylesheet from their own pipeline.
