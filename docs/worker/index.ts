import { createAEOWorker, type MinimalEnv } from '@dualmark/cloudflare';

const upstream = {
  fetch: (request: Request, env: MinimalEnv) => env.ASSETS.fetch(request),
};

// Mirrors next.config.mjs (no trailingSlash) and wrangler html_handling
// "auto-trailing-slash". The three must agree or a bot hits a 301 loop.
export default createAEOWorker({
  upstream,
  trailingSlash: 'never',
});
