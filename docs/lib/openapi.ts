import { createOpenAPI } from 'fumadocs-openapi/server';

/**
 * The docs site consumes the EXPORTED spec (`dronte openapi`, committed at
 * docs/openapi/dronte.yaml by `pnpm generate`) — generated-from-code, like
 * every other contract artifact. It does NOT read specs/openapi.yaml, which
 * is the frozen convergence target, not the published document.
 */
export const openapi = createOpenAPI({
  input: ['./openapi/dronte.yaml'],
});
