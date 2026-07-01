import { CodeBlock } from './code-block';
import { ComingSoon } from './coming-soon';
import { ArrowRight } from './icons';
import { links } from './links';

export function Quickstart() {
  return (
    <section className="border-t border-fd-border bg-fd-muted/30 px-6 py-24">
      <div className="mx-auto max-w-[1000px]">
        <h2 className="chimely-display text-[clamp(2rem,4vw,2.875rem)] leading-[1.08] tracking-[-0.005em] text-fd-foreground">
          Up and running in <span className="italic text-[#1264FF]">two</span> commands.
        </h2>

        <div className="mt-11 grid gap-[18px] [grid-template-columns:repeat(auto-fit,minmax(310px,1fr))]">
          <div className="flex flex-col gap-3.5">
            <CodeBlock
              lang="bash"
              code="npx chimely dev"
              copyLabel="Copy npx command"
              badge={<ComingSoon />}
            >
              <span className="text-[#7EE0A6]">$</span> npx chimely dev
            </CodeBlock>
            <p className="text-sm leading-relaxed text-fd-muted-foreground">
              Embedded Postgres, no Redis required. First notification in under a minute.
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
