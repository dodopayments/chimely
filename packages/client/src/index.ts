/**
 * @dronte/client — headless core (scaffold).
 *
 * The public surface is frozen in specs/sdk-api.d.ts; the implementation
 * (auth, SSE reconnect/resume, inbox store, optimistic updates, pagination)
 * lands in Phase 2. Wire types under ./generated are produced from the
 * server's exported OpenAPI document — never hand-edit them.
 */
export const SDK_NAME = '@dronte/client';
