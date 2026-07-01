import { links } from './links';

export function SiteFooter() {
  return (
    <footer className="border-t border-fd-border bg-fd-background px-6 pb-10 pt-12">
      <div className="mx-auto flex max-w-[1200px] flex-wrap items-start justify-between gap-7">
        <div>
          <div className="flex items-center gap-2">
            <a
              href={links.dodo}
              target="_blank"
              rel="noopener noreferrer"
              aria-label="Dodo Payments"
              className="flex items-center transition-opacity hover:opacity-80"
            >
              {/* biome-ignore lint/performance/noImgElement: inline SVG brand mark; next/image adds no value and keeps the home route framework-portable */}
              <img
                src="/chimely/logo-dodo.svg"
                alt="Dodo Payments"
                width={22}
                height={22}
                className="block size-[22px]"
              />
            </a>
            <span className="text-[17px] text-fd-muted-foreground/60">/</span>
            <span className="text-[17px] font-semibold tracking-tight text-fd-foreground">
              Chimely
            </span>
          </div>
          <p className="mt-3 text-sm text-fd-muted-foreground">
            Maintained and used by the engineering team at{' '}
            <a
              href={links.dodo}
              target="_blank"
              rel="noopener noreferrer"
              className="border-b border-fd-border text-fd-foreground no-underline transition-colors hover:text-[#1264FF]"
            >
              Dodo Payments
            </a>
            .
          </p>
        </div>

        <nav className="flex flex-wrap gap-7" aria-label="Footer">
          <a
            href={links.repo}
            target="_blank"
            rel="noopener noreferrer"
            className="text-sm font-medium text-fd-muted-foreground no-underline transition-colors hover:text-fd-foreground"
          >
            GitHub
          </a>
          <a
            href={links.twitter}
            target="_blank"
            rel="noopener noreferrer"
            className="text-sm font-medium text-fd-muted-foreground no-underline transition-colors hover:text-fd-foreground"
          >
            X / Twitter
          </a>
          <a
            href={links.docs}
            className="text-sm font-medium text-fd-muted-foreground no-underline transition-colors hover:text-fd-foreground"
          >
            Docs
          </a>
        </nav>
      </div>

      {/* Pre-1.0 disclaimer — kept visually quiet */}
      <div className="mx-auto mt-7 max-w-[1200px] border-t border-fd-border pt-5">
        <p className="max-w-[80ch] font-mono text-[12.5px] leading-relaxed text-fd-muted-foreground/70">
          Chimely is at v0.1.0 — the HTTP API and SDK surface may still change on minor releases
          until 1.0. Pin your versions.
        </p>
      </div>
    </footer>
  );
}
