import type { source } from '@/lib/source';

type SourcePage = (typeof source)['$inferPage'];

// Guide pages expose getText from fumadocs-mdx (requires includeProcessedMarkdown
// in source.config.ts). OpenAPI reference pages are virtual and carry no Markdown
// source, so they contribute a title and description stub. Full endpoint detail
// lives in the OpenAPI export.
export async function getLLMText(page: SourcePage): Promise<string> {
  const { title, description } = page.data;
  const heading = `# ${title}\nURL: ${page.url}`;
  const summary = description ? `\n\n${description}` : '';

  const data = page.data as {
    getText?: (type: 'raw' | 'processed') => Promise<string>;
  };
  if (typeof data.getText === 'function') {
    const body = await data.getText('processed');
    return `${heading}${summary}\n\n${body}`;
  }
  return `${heading}${summary}`;
}
