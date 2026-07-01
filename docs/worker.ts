import { createAEOWorker } from '@dualmark/cloudflare';

// Static export served by Cloudflare Workers Static Assets. The Next.js `out/`
// directory is exposed via the ASSETS binding (see wrangler.jsonc). The
// upstream simply forwards to the asset server. createAEOWorker runs first
// (run_worker_first) so content negotiation and markdown twins are served
// before the asset fallback.
interface Env {
  ASSETS: { fetch: (request: Request) => Promise<Response> };
}

const upstream = {
  async fetch(request: Request, env: Env): Promise<Response> {
    return env.ASSETS.fetch(request);
  },
};

export default createAEOWorker({
  upstream,
  // Matches next.config.mjs (no trailingSlash) and wrangler
  // html_handling "auto-trailing-slash".
  trailingSlash: 'never',
  // AEO Spec §6: advertise the markdown twin on every HTML response.
  enableLinkHeader: true,
});
