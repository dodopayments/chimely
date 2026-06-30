import { createMDX } from 'fumadocs-mdx/next';

/** @type {import('next').NextConfig} */
const config = {
  reactStrictMode: true,
  // Static export: the docs site has no server runtime (every route is
  // prerendered), so it ships as static assets served by Cloudflare Workers
  // Static Assets. Pairs with html_handling "auto-trailing-slash" in
  // wrangler.jsonc.
  output: 'export',
  // next/image optimization needs a server; static export requires the
  // unoptimized loader. The docs use no <Image> today, this keeps it safe.
  images: { unoptimized: true },
};

const withMDX = createMDX();

export default withMDX(config);
