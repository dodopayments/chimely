import { getLLMText } from '@/lib/get-llm-text';
import { source } from '@/lib/source';

// Static export: prerender the concatenated corpus at build time.
export const dynamic = 'force-static';
export const revalidate = false;

export async function GET() {
  const pages = source.getPages();
  const text = (await Promise.all(pages.map(getLLMText))).join('\n\n');

  return new Response(text, {
    headers: { 'Content-Type': 'text/plain; charset=utf-8' },
  });
}
