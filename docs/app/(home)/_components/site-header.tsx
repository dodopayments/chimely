'use client';

// useSearchContext is provided by Fumadocs' <RootProvider> (in the root layout).
// It opens the standard ⌘K search dialog. In fumadocs-ui 16 the hook is exported
// from the contexts/search subpath.
import { useSearchContext } from 'fumadocs-ui/contexts/search';
import { useTheme } from 'next-themes';
import { useEffect, useState } from 'react';
import { ArrowRight, GitHubIcon, MoonIcon, SearchIcon, SunIcon } from './icons';
import { links } from './links';

export function SiteHeader() {
  const { resolvedTheme, setTheme } = useTheme();
  const search = useSearchContext();
  const [mounted, setMounted] = useState(false);

  // Avoid hydration mismatch on the theme icon.
  useEffect(() => setMounted(true), []);

  return (
    <header className="sticky top-0 z-50 border-b border-fd-border bg-fd-background/75 backdrop-blur-md">
      <div className="mx-auto flex h-[60px] max-w-[1200px] items-center justify-between gap-3 px-4 sm:gap-3.5 sm:px-5">
        {/* Org-namespaced wordmark: Dodo Payments logo + breadcrumb */}
        <div className="flex min-w-0 items-center gap-2 overflow-hidden whitespace-nowrap">
          <a
            href={links.dodo}
            target="_blank"
            rel="noopener noreferrer"
            aria-label="Dodo Payments"
            className="hidden items-center gap-2 no-underline transition-opacity hover:opacity-80 sm:flex"
          >
            {/* Authentic Dodo Payments mark, see public/chimely/logo-dodo.svg */}
            {/* biome-ignore lint/performance/noImgElement: inline SVG brand mark; next/image adds no value and keeps the home route framework-portable */}
            <img
              src="/chimely/logo-dodo.svg"
              alt="Dodo Payments"
              width={22}
              height={22}
              className="block size-[22px]"
            />
          </a>
          <span className="hidden text-[15px] text-fd-muted-foreground/60 sm:inline">/</span>
          <a href={links.docs} className="no-underline transition-opacity hover:opacity-80">
            <span className="text-[15px] font-semibold tracking-tight text-fd-foreground">
              Chimely
            </span>
          </a>
        </div>

        <div className="flex items-center gap-2 sm:gap-2.5">
          <button
            type="button"
            onClick={() => search.setOpenSearch(true)}
            aria-label="Search documentation (Command K)"
            className="flex h-[34px] items-center gap-2 rounded-lg border border-fd-border bg-fd-muted/40 pl-2.5 pr-2 text-[13px] text-fd-muted-foreground transition-colors hover:border-[#1264FF] hover:text-fd-foreground"
          >
            <SearchIcon className="size-[15px]" />
            <span className="hidden sm:inline">Search docs</span>
            <kbd className="hidden rounded border border-fd-border px-1.5 py-px font-mono text-[11px] leading-snug text-fd-muted-foreground/70 sm:inline-block">
              ⌘K
            </kbd>
          </button>

          <button
            type="button"
            onClick={() => setTheme(resolvedTheme === 'dark' ? 'light' : 'dark')}
            aria-label="Toggle color theme"
            className="grid size-[34px] place-items-center rounded-lg border border-fd-border text-fd-muted-foreground transition-colors hover:border-[#1264FF] hover:text-fd-foreground"
          >
            {mounted && resolvedTheme === 'dark' ? (
              <MoonIcon className="size-4" />
            ) : (
              <SunIcon className="size-4" />
            )}
          </button>

          <a
            href={links.repo}
            target="_blank"
            rel="noopener noreferrer"
            aria-label="Star Chimely on GitHub"
            className="hidden h-[34px] items-center gap-1.5 rounded-lg border border-fd-border px-3 text-[13px] font-medium text-fd-foreground no-underline transition-colors hover:border-[#1264FF] sm:inline-flex"
          >
            <GitHubIcon className="size-[15px]" />
            <span className="hidden sm:inline">Star</span>
          </a>

          <a
            href={links.docs}
            className="inline-flex h-[34px] items-center gap-1.5 whitespace-nowrap rounded-lg bg-[#1264FF] px-3 text-[13px] font-semibold text-white no-underline transition-colors hover:bg-[#0b53e0] sm:px-3.5"
          >
            Get Started
            <ArrowRight className="hidden size-3.5 sm:block" />
          </a>
        </div>
      </div>
    </header>
  );
}
