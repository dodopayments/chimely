# Chimely quickstart (Next.js)

The drop-in `<Inbox />` running against a local chimely in Redis-less mode
(hints ride Postgres LISTEN/NOTIFY — no Redis needed for the quickstart).

## 30-second quickstart

From the repository root:

```bash
# 1. Postgres (skip if you have one; any reachable instance works)
docker run -d --name chimely-quickstart-pg \
  -e POSTGRES_PASSWORD=chimely -p 5432:5432 postgres:16-alpine

# 2. Chimely, with the dev bootstrap: seeds environment `demo`
#    (no subscriber hashes) and the API key `dev-secret-key`
DATABASE_URL=postgres://postgres:chimely@localhost:5432/postgres \
CHIMELY_DEV_ENVIRONMENT=demo \
CHIMELY_DEV_API_KEY=dev-secret-key \
CHIMELY_LISTEN_ADDR=127.0.0.1:8080 \
cargo run --manifest-path server/Cargo.toml -- serve

# 3. This example (new terminal)
pnpm install
pnpm --filter "./packages/*" build
pnpm --filter chimely-example-nextjs dev
```

Open <http://localhost:3000>, then send yourself a notification:

```bash
curl -X POST http://localhost:8080/v1/notifications \
  -H 'Authorization: Bearer dev-secret-key' \
  -H 'Content-Type: application/json' \
  -d '{"subscriber_id":"usr_demo","category":"demo.greeting","payload":{"title":"Hello from curl","body":"This arrived over the SSE hint stream."}}'
```

The bell badge increments live. No page refresh: the server publishes an
SSE hint and the widget refetches conditionally (ETag, mostly 304s).

## Production differences

- Set `subscriberHash` on `<Inbox />` — `hex(HMAC-SHA256(secret,
  environmentId + "\0" + subscriberId))`, computed by **your backend**,
  never in the browser. The environment id is the `env_...` id from the
  admin dashboard.
  The dev bootstrap turns the requirement off; production environments
  keep it on.
- `CHIMELY_DEV_ENVIRONMENT` / `CHIMELY_DEV_API_KEY` are for local
  quickstarts only. Real environments and keys are managed in the admin
  UI (Phase 4).
- Point `NEXT_PUBLIC_CHIMELY_URL`, `NEXT_PUBLIC_CHIMELY_ENVIRONMENT`, and
  `NEXT_PUBLIC_CHIMELY_SUBSCRIBER_ID` at your deployment to reuse this app
  against it.
