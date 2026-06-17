import { createOpenAPI } from 'fumadocs-openapi/server';

/**
 * The docs site consumes the exported spec (`dronte openapi`, committed at
 * docs/openapi/dronte.yaml by `pnpm generate`). Generated from code, never
 * hand-edited.
 */
export const openapi = createOpenAPI({
  input: ['./openapi/dronte.yaml'],
});
