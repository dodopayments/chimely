/**
 * Compile-time conformance of the BUILT @dronte/client (dist/index.d.ts)
 * against the frozen contract. The spec is specs/sdk-api.d.ts, mechanically
 * renamed to @dronte-spec/client by scripts/conformance-spec.mjs so both
 * sides live in one program. Data shapes assert both directions. Classes
 * and functions assert impl-extends-spec, because additive optional
 * members are sanctioned by the contract's stability rules.
 */

import type * as Spec from '@dronte-spec/client';
import type * as Impl from '../packages/client/dist/index.js';

type Assert<T extends true> = T;
type Extends<A, B> = [A] extends [B] ? true : false;
type Mutual<A, B> = Extends<A, B> extends true ? Extends<B, A> : false;

export type ItemSource = Assert<Mutual<Impl.InboxItemSource, Spec.InboxItemSource>>;
export type NotificationId = Assert<Mutual<Impl.NotificationId, Spec.NotificationId>>;
export type BroadcastId = Assert<Mutual<Impl.BroadcastId, Spec.BroadcastId>>;
export type ItemId = Assert<Mutual<Impl.InboxItemId, Spec.InboxItemId>>;
export type Payload = Assert<Mutual<Impl.WellKnownPayload, Spec.WellKnownPayload>>;
export type Item = Assert<Mutual<Impl.InboxItem, Spec.InboxItem>>;
export type ItemTyped = Assert<
  Mutual<Impl.InboxItem<{ amount: number }>, Spec.InboxItem<{ amount: number }>>
>;
export type Counts = Assert<Mutual<Impl.InboxCounts, Spec.InboxCounts>>;
export type Pref = Assert<Mutual<Impl.Preference, Spec.Preference>>;
export type Status = Assert<Mutual<Impl.ConnectionStatus, Spec.ConnectionStatus>>;
export type Backoff = Assert<Mutual<Impl.BackoffConfig, Spec.BackoffConfig>>;
export type Config = Assert<Mutual<Impl.DronteClientConfig, Spec.DronteClientConfig>>;
export type SourceLike = Assert<Mutual<Impl.EventSourceLike, Spec.EventSourceLike>>;
export type Snapshot = Assert<Mutual<Impl.InboxSnapshot, Spec.InboxSnapshot>>;
export type SnapshotTyped = Assert<
  Mutual<Impl.InboxSnapshot<{ amount: number }>, Spec.InboxSnapshot<{ amount: number }>>
>;

// The class, statics and instance side together. The spec declares the
// constructor explicitly, so typeof covers it.
export type Client = Assert<Extends<typeof Impl.DronteClient, typeof Spec.DronteClient>>;
export type ClientTyped = Assert<
  Extends<Impl.DronteClient<{ amount: number }>, Spec.DronteClient<{ amount: number }>>
>;

// DronteError: instance side both ways. The static side is excluded on
// purpose: the spec declares no constructor, which TS widens to the base
// Error construct signature, and the implementation's options-bag
// constructor is intentionally richer.
export type ErrInstance = Assert<Mutual<Impl.DronteError, Spec.DronteError>>;
export type ErrIsError = Assert<Extends<Impl.DronteError, Error>>;
