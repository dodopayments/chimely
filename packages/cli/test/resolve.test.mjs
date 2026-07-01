import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import { resolveBinary } from '../src/resolve.mjs';

const exe = process.platform === 'win32' ? 'chimely.exe' : 'chimely';
const cleanups = [];

afterEach(() => {
  while (cleanups.length > 0) cleanups.pop()();
});

function tempDir() {
  const dir = mkdtempSync(join(tmpdir(), 'chimely-cli-'));
  cleanups.push(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
}

function touch(path) {
  writeFileSync(path, '');
  return path;
}

describe('resolveBinary', () => {
  it('prefers CHIMELY_BIN when the file exists', () => {
    const dir = tempDir();
    const bin = touch(join(dir, 'my-chimely'));
    expect(resolveBinary({ env: { CHIMELY_BIN: bin }, startDir: dir })).toBe(bin);
  });

  it('throws when CHIMELY_BIN points at a missing file', () => {
    expect(() =>
      resolveBinary({ env: { CHIMELY_BIN: join(tempDir(), 'nope') }, startDir: tempDir() }),
    ).toThrow(/CHIMELY_BIN/);
  });

  it('honors CARGO_TARGET_DIR', () => {
    const dir = tempDir();
    const target = join(dir, 'shared-target', 'debug');
    mkdirSync(target, { recursive: true });
    const bin = touch(join(target, exe));
    expect(
      resolveBinary({ env: { CARGO_TARGET_DIR: join(dir, 'shared-target') }, startDir: dir }),
    ).toBe(bin);
  });

  it('finds a monorepo build under server/target, walking up from a nested dir', () => {
    const dir = tempDir();
    const target = join(dir, 'server', 'target', 'debug');
    mkdirSync(target, { recursive: true });
    const bin = touch(join(target, exe));
    const nested = join(dir, 'packages', 'cli');
    mkdirSync(nested, { recursive: true });
    expect(resolveBinary({ env: {}, startDir: nested })).toBe(bin);
  });

  it('throws actionable guidance when nothing is found', () => {
    expect(() => resolveBinary({ env: {}, startDir: tempDir() })).toThrow(
      /Prebuilt binaries are not published/,
    );
  });
});
