import type { Node } from 'fumadocs-core/page-tree';
import { DocsLayout } from 'fumadocs-ui/layouts/notebook';
import { getLayoutTabs } from 'fumadocs-ui/layouts/shared';
import type { ReactNode } from 'react';
import { source } from '@/lib/source';

// The management group is the public write API: keep it expanded by default.
function expandManagement(nodes: Node[]): void {
  for (const node of nodes) {
    if (node.type === 'folder') {
      const isManagement = node.children.some(
        (child) => child.type === 'page' && child.url.startsWith('/docs/api/management/'),
      );
      if (isManagement) node.defaultOpen = true;
      expandManagement(node.children);
    }
  }
}

export default function Layout({ children }: { children: ReactNode }) {
  const tree = source.pageTree;
  // The API reference (`root: true`) lives in the tree's fallback branch.
  if (tree.fallback) expandManagement(tree.fallback.children);

  // The guides are the default root; the API reference is a `root: true`
  // folder. Derive its tab so it carries a `$folder` binding (precise active
  // state, sidebar scopes to the endpoint tree), and force it listed so it
  // shows even while reading the guides. The guides tab is a plain link whose
  // `urls` keep it inactive on /docs/api pages.
  const guideUrls = new Set(
    tree.children
      .filter(
        (node): node is Node & { url: string } =>
          node.type === 'page' && !node.url.startsWith('/docs/api'),
      )
      .map((node) => node.url),
  );
  const tabs = [
    { title: 'Documentation', url: '/docs', urls: guideUrls },
    ...getLayoutTabs(tree, { transform: (tab) => ({ ...tab, unlisted: false }) }),
  ];

  return (
    <DocsLayout
      tree={tree}
      nav={{ title: 'Chimely' }}
      tabMode="navbar"
      tabs={tabs}
      githubUrl="https://github.com/that-ambuj/chimely"
    >
      {children}
    </DocsLayout>
  );
}
