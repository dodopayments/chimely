// Admin API client. Every endpoint is same-origin under /admin/api/* and
// gated by HTTP Basic auth; the browser holds the credential (it prompted on
// the first /admin load) and attaches it to same-origin fetches automatically.

export interface ApiError {
  status: number;
  code: string;
  message: string;
}

export class ApiRequestError extends Error {
  status: number;
  code: string;
  constructor(err: ApiError) {
    super(err.message);
    this.status = err.status;
    this.code = err.code;
  }
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(`/admin/api${path}`, {
    ...init,
    credentials: 'same-origin',
    headers: {
      ...(init?.body ? { 'Content-Type': 'application/json' } : {}),
      ...init?.headers,
    },
  });

  if (res.status === 401) {
    window.dispatchEvent(new CustomEvent('dronte-admin-unauthorized'));
    throw new ApiRequestError({ status: 401, code: 'unauthorized', message: 'Session expired' });
  }

  if (!res.ok) {
    let code = 'error';
    let message = `Request failed (${res.status})`;
    try {
      const body = (await res.json()) as { error?: { code?: string; message?: string } };
      if (body.error) {
        code = body.error.code ?? code;
        message = body.error.message ?? message;
      }
    } catch {
      /* non-JSON error body */
    }
    throw new ApiRequestError({ status: res.status, code, message });
  }

  if (res.status === 204) return undefined as T;
  return (await res.json()) as T;
}

const get = <T,>(path: string) => request<T>(path);
const post = <T,>(path: string, body?: unknown) =>
  request<T>(path, { method: 'POST', body: body === undefined ? undefined : JSON.stringify(body) });

// ----- Types (mirror server/src/api/admin.rs) -------------------------------

export interface AdminEnvironment {
  id: string;
  slug: string;
  name: string;
  require_subscriber_hash: boolean;
  created_at: string;
}

export interface AdminEnvironmentDetail extends AdminEnvironment {
  subscriber_hmac_secret: string;
  has_previous_secret: boolean;
  subscriber_hmac_rotated_at: string | null;
}

export interface AdminHmacRotation {
  subscriber_hmac_secret: string;
  has_previous_secret: boolean;
  subscriber_hmac_rotated_at: string | null;
}

export interface AdminApiKey {
  id: string;
  name: string;
  key_prefix: string;
  created_at: string;
  last_used_at: string | null;
  revoked_at: string | null;
}

export interface AdminApiKeyCreated {
  id: string;
  name: string;
  key_prefix: string;
  key: string;
  created_at: string;
}

export interface AdminNotification {
  id: string;
  subscriber_id: string;
  category: string;
  payload: Record<string, unknown>;
  created_at: string;
  deliver_at: string | null;
  visible_at: string;
  read_at: string | null;
}

export interface AdminNotificationPage {
  items: AdminNotification[];
  next_cursor: string | null;
}

export interface AdminTimelineEntry {
  status: string;
  occurred_at: string;
}

export interface AdminNotificationTimeline {
  id: string;
  subscriber_id: string;
  timeline: AdminTimelineEntry[];
}

export interface AdminPreference {
  category: string;
  channel: string;
  enabled: boolean;
}

export interface AdminInboxItem {
  id: string;
  source: string;
  category: string;
  payload: Record<string, unknown>;
  occurred_at: string;
  read: boolean;
}

export interface AdminCounts {
  unread: number;
  unseen: number;
}

export interface AdminSubscriberView {
  subscriber_id: string;
  created_at: string;
  counters: AdminCounts;
  read_watermark: string;
  seen_watermark: string;
  preferences: AdminPreference[];
  inbox: AdminInboxItem[];
}

export interface AdminDeadLetter {
  id: string;
  environment_slug: string;
  job_type: string;
  attempts: number;
  last_error: string;
  parked_at: string;
}

export interface AdminReplayResult {
  replayed: number;
}

export interface Broadcast {
  id: string;
  category: string;
  payload: Record<string, unknown>;
  created_at: string;
  idempotency_key: string;
}

// ----- Endpoints ------------------------------------------------------------

const enc = encodeURIComponent;

export interface NotificationFilter {
  subscriber_id?: string;
  category?: string;
  after?: string;
  before?: string;
  limit?: number;
  cursor?: string;
}

export const api = {
  listEnvironments: () => get<AdminEnvironment[]>('/environments'),
  createEnvironment: (body: { slug: string; name: string; require_subscriber_hash: boolean }) =>
    post<AdminEnvironmentDetail>('/environments', body),
  getEnvironment: (envId: string) => get<AdminEnvironmentDetail>(`/environments/${enc(envId)}`),
  rotateHmac: (envId: string) =>
    post<AdminHmacRotation>(`/environments/${enc(envId)}/hmac/rotate`),
  completeHmacRotation: (envId: string) =>
    post<void>(`/environments/${enc(envId)}/hmac/rotate/complete`),

  listApiKeys: (envId: string) => get<AdminApiKey[]>(`/environments/${enc(envId)}/api-keys`),
  createApiKey: (envId: string, name: string) =>
    post<AdminApiKeyCreated>(`/environments/${enc(envId)}/api-keys`, { name }),
  revokeApiKey: (envId: string, keyId: string) =>
    post<void>(`/environments/${enc(envId)}/api-keys/${enc(keyId)}/revoke`),

  listNotifications: (envId: string, filter: NotificationFilter) => {
    const q = new URLSearchParams();
    if (filter.subscriber_id) q.set('subscriber_id', filter.subscriber_id);
    if (filter.category) q.set('category', filter.category);
    if (filter.after) q.set('after', filter.after);
    if (filter.before) q.set('before', filter.before);
    if (filter.limit != null) q.set('limit', String(filter.limit));
    if (filter.cursor) q.set('cursor', filter.cursor);
    const qs = q.toString();
    return get<AdminNotificationPage>(
      `/environments/${enc(envId)}/notifications${qs ? `?${qs}` : ''}`,
    );
  },
  notificationTimeline: (envId: string, notifId: string) =>
    get<AdminNotificationTimeline>(
      `/environments/${enc(envId)}/notifications/${enc(notifId)}/timeline`,
    ),

  createBroadcast: (
    envId: string,
    body: { category: string; payload?: Record<string, unknown>; idempotency_key?: string },
  ) => post<Broadcast>(`/environments/${enc(envId)}/broadcasts`, body),

  getSubscriber: (envId: string, subscriberId: string) =>
    get<AdminSubscriberView>(`/environments/${enc(envId)}/subscribers/${enc(subscriberId)}`),

  listDlq: () => get<AdminDeadLetter[]>('/dlq'),
  replayDeadLetter: (jobId: string) => post<AdminReplayResult>(`/dlq/${enc(jobId)}/replay`),
  replayAllDeadLetters: () => post<AdminReplayResult>('/dlq/replay-all'),
};
