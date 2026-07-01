import { CodeBlock } from './code-block';
import { ArrowRight } from './icons';
import { links } from './links';

export function Quickstart() {
  return (
    <section className="border-t border-fd-border bg-fd-muted/30 px-6 py-24">
      <div className="mx-auto max-w-[1000px]">
        <h2 className="text-[40px] font-semibold leading-[1.1] tracking-[-0.03em] text-fd-foreground">
          Up and running in two commands.
        </h2>

        <div className="mt-11 grid gap-[18px] [grid-template-columns:repeat(auto-fit,minmax(310px,1fr))]">
          <div className="flex flex-col gap-3.5">
            <CodeBlock lang="bash" code="npx chimely dev" copyLabel="Copy npx command">
              <span className="text-[#7EE0A6]">$</span> npx chimely dev
            </CodeBlock>
            <p className="flex flex-wrap items-center gap-x-2 gap-y-1 text-sm leading-relaxed text-fd-muted-foreground">
              <span className="rounded-md border border-fd-border bg-fd-muted/60 px-2 py-0.5 font-mono text-[11px] font-medium tracking-wide text-fd-muted-foreground">
                Coming soon
              </span>
              Embedded Postgres, no Redis required. Use Docker Compose today.
            </p>
          </div>

          <div className="flex flex-col gap-3.5">
            <CodeBlock lang="bash" code="docker compose up" copyLabel="Copy docker command">
              <span className="text-[#7EE0A6]">$</span> docker compose up
            </CodeBlock>
            <p className="text-sm leading-relaxed text-fd-muted-foreground">
              Chimely + Postgres + Redis, the way you&apos;ll run it for real.
            </p>
          </div>
        </div>

        <a
          href={links.quickstart}
          className="mt-8 inline-flex items-center gap-1.5 text-[15px] font-semibold text-[#1264FF] no-underline transition-colors hover:text-[#004F32]"
        >
          Read the quickstart <ArrowRight className="size-4" />
        </a>
      </div>
    </section>
  );
}
