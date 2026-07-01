import { defineConfig, defineDocs } from 'fumadocs-mdx/config';

export const docs = defineDocs({
  dir: 'content/docs',
  // Exposes processed Markdown via page.data.getText('processed') for llms-full.txt.
  docs: {
    postprocess: { includeProcessedMarkdown: true },
  },
});

export default defineConfig();
