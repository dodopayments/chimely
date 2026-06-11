import type { components } from './generated/api';

type ErrorEnvelope = components['schemas']['Error'];

export class DronteError extends Error {
  /**
   * Machine-readable code from the server error envelope, or a client-side
   * code ('network', 'unauthorized', ...).
   */
  readonly code: string;
  /** HTTP status when the error came from the server. */
  readonly status?: number;

  constructor(message: string, options: { code: string; status?: number; cause?: unknown }) {
    super(message, options.cause === undefined ? undefined : { cause: options.cause });
    this.name = 'DronteError';
    this.code = options.code;
    if (options.status !== undefined) {
      this.status = options.status;
    }
  }
}

function fallbackCode(status: number): string {
  switch (status) {
    case 401:
      return 'unauthorized';
    case 404:
      return 'not_found';
    case 429:
      return 'rate_limited';
    default:
      return status >= 500 ? 'server_error' : 'invalid_request';
  }
}

/** Builds a DronteError from a non-2xx response, reading the error envelope when present. */
export async function errorFromResponse(response: Response): Promise<DronteError> {
  let code = fallbackCode(response.status);
  let message = `request failed with status ${response.status}`;
  try {
    const body = (await response.json()) as Partial<ErrorEnvelope>;
    if (body?.error && typeof body.error.code === 'string') {
      code = body.error.code;
      message = body.error.message ?? message;
    }
  } catch {
    // Envelope absent or unparsable. The status-derived code stands.
  }
  return new DronteError(message, { code, status: response.status });
}

/** Wraps a thrown fetch failure (DNS, refused connection, abort) as a network error. */
export function networkError(cause: unknown): DronteError {
  const message = cause instanceof Error ? cause.message : 'network request failed';
  return new DronteError(message, { code: 'network', cause });
}
