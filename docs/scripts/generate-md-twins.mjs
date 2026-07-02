// Generates Dualmark markdown twins into the static export (out/).
//
// The @dualmark/cloudflare adapter does not convert HTML at the edge. On an AI
// request it fetches toMarkdownPath(path) from the ASSETS binding and serves it
// verbatim, so the twin files must exist in out/ next to their .html pages.
// Path mapping mirrors @dualmark/core toMarkdownPath: /docs/quickstart ->
// out/docs/quickstart.md, /docs -> out/docs.md, / -> out/index.md.

import {
  readFileSync,
  writeFileSync,
  mkdirSync,
  readdirSync,
  existsSync,
} from 'node:fs';
import { join, dirname, relative, sep, posix } from 'node:path';

const CONTENT_DIR = 'content/docs';
const OUT_DIR = 'out';
const DOCS_BASE = '/docs';

if (!existsSync(OUT_DIR)) {
  throw new Error(`${OUT_DIR}/ not found. Run "next build" before this script.`);
}

// @dualmark/core toMarkdownPath, replicated so the twin path matches what the
// Worker requests.
function toMarkdownPath(pathname) {
  if (pathname.endsWith('.md')) return pathname;
  const trimmed = pathname.replace(/\/+$/, '');
  if (trimmed === '') return '/index.md';
  return `${trimmed}.md`;
}

function readMeta() {
  const src = readFileSync('app/layout.tsx', 'utf8');
  const title = src.match(/title:\s*['"](.+?)['"]/)?.[1] ?? 'Docs';
  const description = src.match(/description:\s*['"](.+?)['"]/)?.[1] ?? '';
  return { title, description };
}

function parseFrontmatter(src) {
  if (!src.startsWith('---')) return { data: {}, body: src };
  const end = src.indexOf('\n---', 3);
  if (end === -1) return { data: {}, body: src };
  const data = {};
  for (const line of src.slice(3, end).trim().split('\n')) {
    const m = line.match(/^([A-Za-z0-9_-]+):\s*(.*)$/);
    if (m) data[m[1]] = m[2].replace(/^['"]|['"]$/g, '').trim();
  }
  return { data, body: src.slice(end + 4).replace(/^\s*\n/, '') };
}

// MDX ESM statements are not valid markdown. JSX components are left inline.
function stripMdx(body) {
  return body
    .split('\n')
    .filter((l) => !/^\s*import\s.+from\s/.test(l) && !/^\s*export\s/.test(l))
    .join('\n')
    .replace(/\n{3,}/g, '\n\n')
    .trim();
}

function collectMdx(dir) {
  const files = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) files.push(...collectMdx(full));
    else if (entry.name.endsWith('.mdx')) files.push(full);
  }
  return files;
}

function routeFor(file) {
  const noExt = relative(CONTENT_DIR, file).replace(/\.mdx$/, '');
  const segs = noExt.split(sep).filter((s) => s !== 'index');
  return posix.join(DOCS_BASE, ...segs);
}

function write(twinPath, content) {
  const fsPath = join(OUT_DIR, twinPath);
  mkdirSync(dirname(fsPath), { recursive: true });
  writeFileSync(fsPath, content.endsWith('\n') ? content : `${content}\n`);
  return twinPath;
}

const meta = readMeta();
const pages = [];

for (const file of collectMdx(CONTENT_DIR)) {
  const route = routeFor(file);
  const { data, body } = parseFrontmatter(readFileSync(file, 'utf8'));
  const title = data.title ?? route.split('/').pop();
  let md = `# ${title}\n\n`;
  if (data.description) md += `> ${data.description}\n\n`;
  md += stripMdx(body);
  const twin = write(toMarkdownPath(route), md);
  pages.push({ route, title, description: data.description ?? '', twin });
}

pages.sort((a, b) => a.route.localeCompare(b.route));

// Homepage twin (/ -> /index.md). The root is a React page with no markdown
// source, so compose an overview plus links to every doc twin.
const links = pages
  .map((p) => `- [${p.title}](${toMarkdownPath(p.route)})${p.description ? `: ${p.description}` : ''}`)
  .join('\n');
const home = `# ${meta.title}\n\n> ${meta.description}\n\n## Documentation\n\n${links}\n`;
write('/index.md', home);

// llms.txt and llms-full.txt are owned by the Fumadocs route handlers under
// app/, so this script does not write them.

console.log(`Generated ${pages.length + 1} markdown twins in ${OUT_DIR}/`);
for (const p of pages) console.log(`  ${p.route}  ->  ${p.twin}`);
console.log('  /            ->  /index.md');
