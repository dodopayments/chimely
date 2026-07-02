---
"@chimely/react": minor
---

Popover robustness: opt-in `portal` rendering to `document.body` with fixed positioning (escapes overflow and transform ancestors), `placementOffset`, controlled `open`/`onOpenChange`, and `routerPush` for SPA navigation on same-origin action URLs. `react-dom` is now a peer dependency. markAllSeen fires on every open transition, including controlled and programmatic opens.
