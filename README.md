# Dronte

Open-source, self-hostable **in-app notification inbox infrastructure**.
One Rust binary + Postgres + Redis, a deliberately small HTTP API, and a
drop-in `<Inbox />` React component. No workflow engine — the inbox is the
primitive.

> Early scaffolding. The plan lives in `docs/dronte-project-plan.md`; the
> frozen v1 contracts in `specs/`; the per-phase specs in
> `specs/phase-*.md`.

## Repository layout

```
server/            Rust binary: API, SSE, workers          (AGPL-3.0-only)
packages/client/   @dronte/client — headless TS core       (MIT)
packages/react/    @dronte/react  — hooks + <Inbox />      (MIT)
examples/          quickstarts and integration examples    (MIT)
docs/              Fumadocs site + project plan            (MIT)
specs/             frozen v1 contracts (read-only)
```

## License FAQ

**What is licensed how?** The server (`server/`) is
[AGPL-3.0-only](./LICENSE). The SDKs (`packages/client`, `packages/react`)
and everything in `examples/` are MIT — they embed in your frontend, and
they carry their own `LICENSE` files.

**Does the AGPL affect my application?** No. Self-hosters run the Dronte
binary as a standalone network service and integrate over HTTP through the
MIT-licensed SDKs. AGPL obligations attach to the server process, not to
code that talks to it — they never reach your codebase. The AGPL exists to
stop a cloud vendor from wrapping Dronte into a closed hosted product, not
to constrain users.

**Do I have to publish my backend because it calls Dronte's API?** No.
Calling the HTTP API is not linking and creates no obligations.

**What if I modify the server?** If you run a modified server for others
over a network, the AGPL requires offering them the modified source —
that's the network-copyleft point of it. Internal unmodified use requires
nothing.

**What about the API spec and docs?** The generated OpenAPI document and
the documentation content are MIT, so third-party clients, bindings, and
integrations are unambiguous.

**The name and logo?** Not licensed. "Dronte" and the logo remain with the
project regardless of code license — that, not the code license, is the
protection against confusing forks.

**Contributing:** DCO from day one — sign your commits (`git commit -s`).
CI enforces the `Signed-off-by` trailer.
