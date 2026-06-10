# Dronte

Fair-source, self-hostable **in-app notification inbox infrastructure**.
One Rust binary + Postgres + Redis, a deliberately small HTTP API, and a
drop-in `<Inbox />` React component. No workflow engine — the inbox is the
primitive.

> Early scaffolding. The plan lives in `docs/dronte-project-plan.md`; the
> frozen v1 contracts in `specs/`; the per-phase specs in
> `specs/phase-*.md`.

## Repository layout

```
server/            Rust binary: API, SSE, workers          (FSL-1.1-MIT)
packages/client/   @dronte/client — headless TS core       (MIT)
packages/react/    @dronte/react  — hooks + <Inbox />      (MIT)
examples/          quickstarts and integration examples    (MIT)
docs/              Fumadocs site + project plan            (MIT)
specs/             frozen v1 contracts (read-only)
```

## License FAQ

**What is licensed how?** The server (`server/`) is
[FSL-1.1-MIT](./LICENSE) — the Functional Source License. The SDKs
(`packages/client`, `packages/react`) and everything in `examples/` are
MIT — they embed in your frontend, and they carry their own `LICENSE`
files.

**What does FSL mean for me?** You can use, self-host, modify, and
redistribute Dronte freely — internally, in production, commercially, at
any scale, for free, forever. The single thing the license prohibits is
offering Dronte itself to others as a competing commercial product or
hosted service. Each release additionally converts to plain MIT two years
after it ships.

**Does the server license affect my application?** No. You run the Dronte
binary as a standalone network service and integrate over HTTP through the
MIT-licensed SDKs. Nothing about the server's license reaches your
codebase, and calling the HTTP API creates no obligations.

**Is this open source?** Not by the OSI definition — FSL is
[fair source](https://fair.io). The source is public, self-hosting is
unrestricted, and every release becomes MIT (true open source) on its
second anniversary.

**What about the API spec and docs?** The generated OpenAPI document and
the documentation content are MIT, so third-party clients, bindings, and
integrations are unambiguous.

**The name and logo?** Not licensed. "Dronte" and the logo remain with the
project regardless of code license — that, not the code license, is the
protection against confusing forks.

**Contributing:** DCO from day one — sign your commits (`git commit -s`);
CI enforces the `Signed-off-by` trailer. External code contributions also
require a CLA (so the licensing model above stays enforceable).
