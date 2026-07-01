# @chimely/cli

Zero-config local launcher for [Chimely](https://github.com/dodopayments/chimely).

```bash
npx chimely dev
```

Boots a throwaway Chimely server backed by an **embedded Postgres** and **no
Redis** (hints ride Postgres `LISTEN/NOTIFY`). It seeds a `dev` environment with
a copy-pasteable API key and a root admin, prints a banner with the server URL
and credentials, and discards the database on exit.

## How it works

`chimely` is a thin wrapper. It locates the `chimely` server binary and execs
`chimely dev`, forwarding arguments, stdio, and signals. The binary is resolved
in this order:

1. `CHIMELY_BIN` — an explicit path to a `chimely` binary.
2. `CARGO_TARGET_DIR/{release,debug}/chimely` — a contributor's shared target.
3. `<repo>/server/target/{release,debug}/chimely` — a monorepo dev build.

> Prebuilt per-platform binaries are not published yet. Until they are, build
> from source with `cargo build --features dev` (inside `server/`), or point
> `CHIMELY_BIN` at a binary. The `dev` subcommand requires a build compiled with
> `--features dev`.

## Not for production

`chimely dev` is a local convenience. Run the real server (`chimely serve`) with
your own Postgres via Docker or your platform of choice.
