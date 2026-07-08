import { defineConfig } from 'tsup';

export default defineConfig({
  entry: ['src/index.ts'],
  format: ['esm', 'cjs'],
  dts: true,
  sourcemap: true,
  clean: true,
  external: ['react', 'react-dom'],
  // The components are client-only (createContext at module scope). The
  // banner keeps the package importable from a React Server Component
  // module graph without every consumer wrapping it.
  banner: { js: "'use client';" },
});
