# Chimely server

The [Chimely](https://github.com/dodopayments/chimely) server: open-source,
self-hostable in-app notification inbox infrastructure in one binary. Postgres
is the source of truth; Redis is an optional real-time hint plane. The image
bundles the operator dashboard at `/admin`.

Published image: `ghcr.io/dodopayments/chimely`

## Run it

Two containers, no Redis (hints fall back to Postgres `LISTEN/NOTIFY`):

```bash
docker network create chimely

docker run -d --name chimely-pg --network chimely \
  -e POSTGRES_PASSWORD=chimely postgres:16-alpine

docker run -d --name chimely --network chimely -p 8080:8080 \
  -e DATABASE_URL=postgres://postgres:chimely@chimely-pg:5432/postgres \
  ghcr.io/dodopayments/chimely:0.2.0
```

Migrations run on boot under an advisory lock. `DATABASE_URL` is the only
required setting; every other knob is an environment variable with a
production default. The image is production-ready as started above: dev
bootstrap (`CHIMELY_DEV_*`) and the recovery admin (`CHIMELY_ADMIN_*`) are
strictly opt-in, the process runs as a non-root user, and logs are JSON.

- Full configuration reference: https://chimely.dev/docs/self-hosting
- Metrics, alerts, dead letters, shutdown: https://chimely.dev/docs/operations
- Probes: `GET /healthz` (liveness), `GET /readyz` (readiness; Redis down is
  not readiness-fatal)
- Production compose with a pinned image:
  [`deploy/docker-compose.yml`](https://github.com/dodopayments/chimely/blob/main/deploy/docker-compose.yml)

Terminate TLS at a reverse proxy in front of the binary; it serves plain HTTP.

## Requirements

- Postgres 15 or newer (the durable state; back this up)
- Redis 7 (optional; holds only recomputable hints and cached counters)

## License

AGPL-3.0. Free to use, self-host, modify, and redistribute at any scale.
Applications integrate over HTTP through the MIT-licensed SDKs
([`@chimely/client`](https://www.npmjs.com/package/@chimely/client),
[`@chimely/react`](https://www.npmjs.com/package/@chimely/react)), so the
server's copyleft never reaches your code. See the
[License FAQ](https://github.com/dodopayments/chimely#license-faq).
