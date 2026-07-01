import { createAEOWorker } from '@dualmark/cloudflare';

// The docs are a Next.js static export served by the Cloudflare Static Assets
// binding. Dualmark runs first (wrangler.jsonc run_worker_first), inspects the
// request, and serves markdown twins to AI agents. Everything else falls
// through to the static assets unchanged.
interface Env {
  ASSETS: { fetch: (request: Request) => Promise<Response> };
}

const upstream = {
  fetch: (request: Request, env: Env) => env.ASSETS.fetch(request),
};

// trailingSlash mirrors next.config.mjs (no trailingSlash) and the wrangler
// html_handling "auto-trailing-slash" normalization.
export default createAEOWorker({
  upstream,
  trailingSlash: 'never',
});
