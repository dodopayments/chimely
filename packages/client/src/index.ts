/**
 * @chimely/client, the framework-agnostic headless core.
 *
 * The public surface is stable and additive-only. Wire types under ./generated
 * are produced from the server's exported OpenAPI document. Never hand-edit
 * them.
 */

export { BACKOFF_DEFAULTS } from './backoff';
export { ChimelyClient } from './client';
export { ChimelyError } from './errors';
export type {
  BackoffConfig,
  BroadcastId,
  ChimelyClientConfig,
  ConnectionStatus,
  EventSourceLike,
  InboxCounts,
  InboxFilterView,
  InboxItem,
  InboxItemId,
  InboxItemSource,
  InboxSnapshot,
  NotificationId,
  PayloadAction,
  Preference,
  WellKnownPayload,
} from './types';
