# @chimely/client

Headless, framework-agnostic core for [Chimely](https://github.com/dodopayments/chimely),
the open-source, self-hostable in-app notification inbox. Zero runtime
dependencies. If you use React, you probably want
[`@chimely/react`](https://www.npmjs.com/package/@chimely/react), which wraps
this client in hooks and a drop-in `<Inbox />`.

```bash
npm install @chimely/client
```

## Usage

```ts
import { ChimelyClient } from '@chimely/client';

const client = new ChimelyClient({
  serverUrl: 'https://chimely.example.com',
  environment: 'production',
  subscriberId: 'usr_123',
  // Computed by YOUR backend, never in the browser:
  // hex(HMAC-SHA256(environment_secret, environmentId + "\0" + subscriberId)),
  // where environmentId is the env_... id from the admin dashboard.
  subscriberHash,
});

const unsubscribe = client.subscribe(() => {
  const { items, counts } = client.getSnapshot();
  render(items, counts.unread);
});

client.connect();
```

Hashes minted with the legacy input (`subscriberId` alone) are still accepted;
that fallback is removed in an announced minor release. See
[Auth and the subscriber hash](https://chimely.dev/docs/auth).

The snapshot is immutable with a new identity per change, so it plugs straight
into `useSyncExternalStore` or any equality-based renderer. Mutations
(`markRead`, `markUnread`, `archive`, `markAllRead`, ...) apply optimistically
and roll back on failure.

Live updates arrive over Server-Sent Events, but SSE is a hint, not a
transport: every hint triggers a conditional REST refetch (ETag, mostly
`304`s), so a missed hint is harmless and reconnects are cheap.

## Types are generated

The wire types are generated from the Chimely server's exported OpenAPI
document and shipped with the package. They are never hand-edited, so the
types you compile against are exactly what the server serves.

## Versioning

Pre-1.0: the HTTP API and this package's surface may change on minor bumps.
Pin your versions.

## Links

- [Documentation](https://chimely.dev/docs)
- [SDK reference](https://chimely.dev/docs/sdk-reference)
- [Auth and the subscriber hash](https://chimely.dev/docs/auth)
- [GitHub](https://github.com/dodopayments/chimely)

## License

MIT. The Chimely server is AGPL-3.0, which does not affect applications that
talk to it over HTTP through this SDK. See the
[License FAQ](https://github.com/dodopayments/chimely#license-faq).
