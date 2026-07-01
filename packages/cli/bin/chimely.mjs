#!/usr/bin/env node
import { spawn } from 'node:child_process';
import { resolveBinary } from '../src/resolve.mjs';

// Thin launcher: find the chimely server binary and hand off to it, forwarding
// arguments, stdio, and termination signals. `npx chimely` with no argument
// defaults to `dev` (the zero-config local server).
function main() {
  let binary;
  try {
    binary = resolveBinary();
  } catch (error) {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
    process.exit(1);
    return;
  }

  const args = process.argv.slice(2);
  const forwarded = args.length > 0 ? args : ['dev'];

  const child = spawn(binary, forwarded, { stdio: 'inherit' });

  for (const signal of ['SIGINT', 'SIGTERM']) {
    process.on(signal, () => {
      if (child.exitCode === null) child.kill(signal);
    });
  }

  child.on('error', (error) => {
    process.stderr.write(`failed to launch chimely: ${error.message}\n`);
    process.exit(1);
  });
  child.on('exit', (code, signal) => {
    if (signal) {
      process.kill(process.pid, signal);
      return;
    }
    process.exit(code ?? 0);
  });
}

main();
