import { CodeBlock } from './code-block';
import { ArrowRight } from './icons';
import { links } from './links';

// The full server block from the docs quickstart, verbatim, so the copy
// button hands over something that actually runs.
const SERVER_CMDS = `docker network create chimely

docker run -d --name chimely-pg --network chimely \\
  -e POSTGRES_PASSWORD=chimely postgres:16-alpine

docker run -d --name chimely --network chimely -p 8080:8080 \\
  --restart unless-stopped \\
  -e DATABASE_URL=postgres://postgres:chimely@chimely-pg:5432/postgres \\
  -e CHIMELY_DEV_ENVIRONMENT=demo \\
  -e CHIMELY_DEV_API_KEY=dev-secret-key \\
  ghcr.io/dodopayments/chimely:0.2.1`;

export function Quickstart() {
  return (
    <section className="border-t border-fd-border bg-fd-muted/30 px-6 py-24">
      <div className="mx-auto max-w-[1000px]">
        <h2 className="chimely-display text-[clamp(2rem,4vw,2.875rem)] leading-[1.08] tracking-[-0.005em] text-fd-foreground">
          Up and running in <span className="italic text-[#1264FF]">five</span> minutes.
        </h2>

        <div className="mt-11 grid gap-[18px] [grid-template-columns:repeat(auto-fit,minmax(310px,1fr))]">
          <div className="flex flex-col gap-3.5">
            <CodeBlock lang="bash" code={SERVER_CMDS} copyLabel="Copy server commands">
              <span className="block">
                <span className="text-[#7EE0A6]">$</span> docker network create chimely
              </span>
              <span className="block">
                <span className="text-[#7EE0A6]">$</span>{' '}docker run &hellip; postgres:16-alpine
              </span>
              <span className="block">
                <span className="text-[#7EE0A6]">$</span>{' '}docker run &hellip;
                ghcr.io/dodopayments/chimely
              </span>
            </CodeBlock>
            <p className="text-sm leading-relaxed text-fd-muted-foreground">
              The published image plus Postgres. Redis optional, nothing to clone or build.
            </p>
          </div>

          <div className="flex flex-col gap-3.5">
            <CodeBlock
              lang="bash"
              code="npm install @chimely/react"
              copyLabel="Copy install command"
            >
              <span className="text-[#7EE0A6]">$</span> npm install @chimely/react
            </CodeBlock>
            <p className="text-sm leading-relaxed text-fd-muted-foreground">
              Drop <code className="font-mono text-[0.9em]">&lt;Inbox /&gt;</code> into your app:
              live badge, SSE updates, read state, out of the box.
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
