import { llms } from 'fumadocs-core/source';
import { source } from '@/lib/source';

// Static export: prerender the index at build time.
export const dynamic = 'force-static';
export const revalidate = false;

export function GET() {
  const helper = llms(source);
  const tree = source.pageTree;
  const parts = [helper.index()];

  // index() only walks the default root. The OpenAPI reference is a root:true
  // tab parked in the fallback branch (an "API reference" folder).
  const fallback = tree.fallback;
  if (fallback) {
    parts.push('');
    for (const node of fallback.children) parts.push(helper.indexNode(node));
  }

  return new Response(parts.join('\n'), {
    headers: { 'Content-Type': 'text/plain; charset=utf-8' },
  });
}
