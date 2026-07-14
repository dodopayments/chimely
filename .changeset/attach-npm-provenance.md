---
"@chimely/client": patch
"@chimely/react": patch
---

Attach npm provenance via `publishConfig.provenance`. The `NPM_CONFIG_PROVENANCE` env did not survive the changesets -> `pnpm publish` chain, so 0.2.1 shipped without `dist.attestations`. Setting it per package is tool-agnostic; the release workflow env stays as belt and braces.
