import { existsSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';

/**
 * Resolve the path to the `chimely` server binary, in priority order:
 *   1. $CHIMELY_BIN                          explicit override
 *   2. $CARGO_TARGET_DIR/{release,debug}     a contributor's shared cargo target
 *   3. <repo>/server/target/{release,debug}  a monorepo dev build (walking up)
 * Returns the first path that exists, or throws with actionable guidance.
 *
 * Prebuilt release binaries are not published yet; once they are, a download
 * step slots in ahead of the monorepo lookups. `env` and `startDir` are
 * injectable so the resolution order is unit-testable without a real tree.
 */
export function resolveBinary({ env = process.env, startDir = process.cwd() } = {}) {
  const override = env.CHIMELY_BIN;
  if (override) {
    if (existsSync(override)) return override;
    throw new Error(`CHIMELY_BIN is set to ${override} but no file exists there.`);
  }

  const exe = process.platform === 'win32' ? 'chimely.exe' : 'chimely';
  const candidates = [];

  if (env.CARGO_TARGET_DIR) {
    for (const profile of ['release', 'debug']) {
      candidates.push(join(env.CARGO_TARGET_DIR, profile, exe));
    }
  }
  for (const dir of ancestors(startDir)) {
    for (const profile of ['release', 'debug']) {
      candidates.push(join(dir, 'server', 'target', profile, exe));
    }
  }

  for (const candidate of candidates) {
    if (existsSync(candidate)) return candidate;
  }

  throw new Error(
    [
      'Could not find the chimely server binary.',
      '',
      'Prebuilt binaries are not published yet. To run the dev server now:',
      '  - build from source:  cargo build --features dev   (run inside server/), or',
      '  - point at a binary:  CHIMELY_BIN=/path/to/chimely npx chimely dev',
    ].join('\n'),
  );
}

/** Yield `start` and each ancestor directory up to and including the filesystem root. */
function* ancestors(start) {
  let dir = resolve(start);
  let parent = dirname(dir);
  while (parent !== dir) {
    yield dir;
    dir = parent;
    parent = dirname(dir);
  }
  yield dir;
}
