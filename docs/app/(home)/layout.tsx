import type { ReactNode } from 'react';
import './home.css';

/**
 * Home-route layout.
 *
 * The standard Fumadocs <RootProvider> (in your root `app/layout.tsx`) already
 * supplies the theme (next-themes) and the ⌘K search context that the page's
 * header uses. Nothing extra is needed here.
 *
 * This page renders its own <SiteHeader>, so we intentionally do NOT mount
 * Fumadocs' <HomeLayout> nav. If you'd rather use Fumadocs' shared nav/footer,
 * wrap {children} in <HomeLayout {...baseOptions}> and remove <SiteHeader/>
 * from page.tsx.
 */
export default function Layout({ children }: { children: ReactNode }) {
  return children;
}
