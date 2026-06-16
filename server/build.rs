//! Guarantees `admin/dist` exists at compile time so the rust-embed derive in
//! `api::admin` always has a folder to embed. The real admin SPA build
//! (`pnpm --filter dronte-admin build`, run by CI and the Dockerfile) writes
//! the production bundle there before `cargo build`. When it has not run — a
//! bare `cargo nextest`, `cargo run -- openapi`, or `cargo sqlx prepare` — a
//! minimal placeholder shell is written so the binary still compiles and
//! serves a valid `/admin` page. The placeholder is overwritten by the real
//! build and never committed (server/admin/dist is gitignored).

use std::fs;
use std::path::Path;

const PLACEHOLDER: &str = r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Dronte admin</title>
  </head>
  <body>
    <main style="font-family: system-ui, sans-serif; max-width: 32rem; margin: 4rem auto; padding: 0 1rem;">
      <h1>Dronte admin</h1>
      <p>The admin dashboard bundle has not been built. Run
      <code>pnpm --filter dronte-admin build</code> and rebuild the server.</p>
    </main>
  </body>
</html>
"#;

fn main() {
    let dist = Path::new("admin/dist");
    let index = dist.join("index.html");
    if !index.exists() {
        fs::create_dir_all(dist).expect("create admin/dist");
        fs::write(&index, PLACEHOLDER).expect("write placeholder admin/dist/index.html");
    }
    // Re-embed when the SPA bundle changes (so a fresh `pnpm build` is picked
    // up) and when this script changes.
    println!("cargo:rerun-if-changed=admin/dist");
    println!("cargo:rerun-if-changed=build.rs");
}
