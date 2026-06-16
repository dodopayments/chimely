/**
 * @dronte/client, the framework-agnostic headless core.
 *
 * The public surface is frozen and additive-only against
 * project/archive-v1/sdk-api.d.ts. Wire types under ./generated are produced
 * from the server's exported OpenAPI document. Never hand-edit them.
 */

export { BACKOFF_DEFAULTS } from './backoff';
export { DronteClient } from './client';
export { DronteError } from './errors';
export type {
  BackoffConfig,
  BroadcastId,
  ConnectionStatus,
  DronteClientConfig,
  EventSourceLike,
  InboxCounts,
  InboxItem,
  InboxItemId,
  InboxItemSource,
  InboxSnapshot,
  NotificationId,
  Preference,
  WellKnownPayload,
} from './types';
