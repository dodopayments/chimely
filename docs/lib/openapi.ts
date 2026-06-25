import { createOpenAPI } from 'fumadocs-openapi/server';

/**
 * The docs site consumes the exported spec (`chimely openapi`, committed at
 * docs/openapi/chimely.yaml by `pnpm generate`). Generated from code, never
 * hand-edited.
 */
export const openapi = createOpenAPI({
  input: ['./openapi/chimely.yaml'],
});
