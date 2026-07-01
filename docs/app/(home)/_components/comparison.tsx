import { ArrowRight, CheckIcon, XIcon } from './icons';
import { links } from './links';

const platform = [
  'A workflow engine + inbox channel',
  'Mongo + several services',
  'A step-based mental model to learn',
];
const chimely = ['One binary', 'One Postgres', 'One POST'];

export function Comparison() {
  return (
    <section className="border-t border-fd-border bg-fd-background px-6 py-24">
      <div className="mx-auto max-w-[1000px] text-center">
        <h2 className="text-[40px] font-semibold leading-[1.1] tracking-[-0.03em] text-fd-foreground">
          Simpler than a notification platform.
        </h2>
        <p className="mx-auto mt-4 max-w-[74ch] text-[17px] leading-[1.62] text-fd-muted-foreground">
          Most notification tools model a workflow engine that happens to have an inbox channel —
          Mongo, several services, and a step-based mental model to learn. Chimely models the inbox
          itself: one binary, one Postgres, one POST. Multi-tenancy is solved the way Plausible
          solves it — run another instance.
        </p>

        <div className="my-10 grid gap-4 text-left [grid-template-columns:repeat(auto-fit,minmax(260px,1fr))]">
          <div className="rounded-2xl border border-fd-border bg-fd-muted/30 p-6">
            <div className="font-mono text-xs uppercase tracking-wider text-fd-muted-foreground">
              A notification platform
            </div>
            <ul className="mt-4 flex flex-col gap-2.5">
              {platform.map((item) => (
                <li
                  key={item}
                  className="flex items-center gap-2.5 text-[15px] text-fd-muted-foreground"
                >
                  <XIcon className="size-4 shrink-0 text-fd-muted-foreground/70" />
                  {item}
                </li>
              ))}
            </ul>
          </div>

          <div className="rounded-2xl border border-[#1264FF] bg-[#1264FF]/[0.07] p-6">
            <div className="font-mono text-xs uppercase tracking-wider text-[#1264FF]">Chimely</div>
            <ul className="mt-4 flex flex-col gap-2.5">
              {chimely.map((item) => (
                <li key={item} className="flex items-center gap-2.5 text-[15px] text-fd-foreground">
                  <CheckIcon className="size-4 shrink-0 text-[#1264FF]" />
                  {item}
                </li>
              ))}
            </ul>
          </div>
        </div>

        <a
          href={links.comparison}
          className="inline-flex items-center gap-1.5 text-[15px] font-semibold text-[#1264FF] no-underline transition-colors hover:text-[#004F32]"
        >
          Chimely vs Novu <ArrowRight className="size-4" />
        </a>
      </div>
    </section>
  );
}
