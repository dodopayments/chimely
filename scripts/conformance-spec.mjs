#!/usr/bin/env node
// Rewrites the frozen SDK contract's ambient module names
// (specs/sdk-api.d.ts) from @dronte/* to @dronte-spec/* so the spec and the
// real packages can be type-checked in one program: the original names
// would clash with the implementations. Mechanical rename only, never a
// transcription. The output is gitignored and regenerated on every check.
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const spec = readFileSync(join(root, 'specs/sdk-api.d.ts'), 'utf8');
const renamed = spec
  .replaceAll("'@dronte/client'", "'@dronte-spec/client'")
  .replaceAll("'@dronte/react'", "'@dronte-spec/react'");
const outDir = join(root, 'conformance/.generated');
mkdirSync(outDir, { recursive: true });
writeFileSync(join(outDir, 'sdk-api-spec.d.ts'), renamed);
