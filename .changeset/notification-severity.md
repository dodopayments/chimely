---
"@chimely/client": minor
"@chimely/react": minor
---

First-class notification severity (frontend slice). `WellKnownPayload` gains an optional `severity: 'high' | 'medium' | 'low'`, and the default item renders a left accent tinted by the new `colorSeverityHigh` / `colorSeverityMedium` / `colorSeverityLow` appearance variables. Any other value renders no accent. Tabs can already filter on `severity` via their `filter` predicate.
