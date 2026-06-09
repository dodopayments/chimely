#!/usr/bin/env bash
# Regenerate every artifact derived from the code-first OpenAPI document.
# CI runs this and fails if `git diff` is non-empty (stale generated files).
#
#   server (utoipa) ──dronte openapi──► docs/openapi/dronte.yaml   (docs site)
#                                  └──► packages/client/src/generated/api.d.ts
#
# Generated outputs are committed; NEVER hand-edit them (see CLAUDE.md).
set -euo pipefail
cd "$(dirname "$0")/.."

cargo run --quiet --manifest-path server/Cargo.toml -- openapi > docs/openapi/dronte.yaml
pnpm exec openapi-typescript docs/openapi/dronte.yaml \
  --output packages/client/src/generated/api.d.ts
