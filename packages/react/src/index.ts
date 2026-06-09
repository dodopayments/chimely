/**
 * @dronte/react — hooks + <Inbox /> (scaffold).
 *
 * The public surface is frozen in specs/sdk-api.d.ts; the implementation
 * (DronteProvider, useNotifications/useUnreadCount/useUnseenCount,
 * usePreferences, <Inbox />) lands in Phase 2 on top of @dronte/client.
 */
import { SDK_NAME as CLIENT_SDK_NAME } from '@dronte/client';

export const SDK_NAME = '@dronte/react';

/** The client core this package is built on. */
export const CORE = CLIENT_SDK_NAME;
