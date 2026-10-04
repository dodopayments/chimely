---
"@chimely/client": minor
"@chimely/react": minor
---

Notification action buttons (frontend slice). `WellKnownPayload` gains optional `primary_action` / `secondary_action` (`{ label, url? }`, exported as `PayloadAction`). The default item renders them as buttons below the content, with new `onPrimaryActionClick` / `onSecondaryActionClick` props and a `renderActions` escape hatch. Action URLs follow the same safe-navigation path as `action_url` (same-origin via `routerPush`, `javascript:`/`data:` refused). No server change: payloads pass through verbatim.
