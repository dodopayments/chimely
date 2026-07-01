import type { ReactNode } from 'react';

/** Thin flow arrow — points right on desktop, down on mobile. */
function FlowArrow() {
  return (
    <svg
      width="34"
      height="14"
      viewBox="0 0 34 14"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.6}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      className="rotate-90 lg:rotate-0"
    >
      <path d="M1 7h30M26 2l6 5-6 5" />
    </svg>
  );
}

function Connector({ label }: { label?: string }) {
  return (
    <div className="flex shrink-0 flex-col items-center gap-1.5 text-fd-muted-foreground">
      {label ? (
        <span className="whitespace-nowrap rounded-md bg-[#1264FF]/10 px-2 py-1 font-mono text-[11px] text-[#1264FF]">
          {label}
        </span>
      ) : null}
      <FlowArrow />
    </div>
  );
}

function Node({ title, sub, accent }: { title: ReactNode; sub: string; accent?: boolean }) {
  return (
    <div
      className={
        'w-full max-w-[260px] rounded-2xl border px-4 py-4 text-center lg:w-auto lg:max-w-[180px] lg:flex-1 ' +
        (accent
          ? 'border-[#1264FF] bg-[#1264FF]/[0.09] shadow-[0_12px_36px_-18px_rgba(18,100,255,0.7)]'
          : 'border-fd-border bg-fd-card')
      }
    >
      <div className="text-[15px] font-semibold text-fd-foreground">{title}</div>
      <div className="mt-1 text-[12.5px] text-fd-muted-foreground">{sub}</div>
    </div>
  );
}

export function Architecture() {
  return (
    <section className="border-t border-fd-border bg-fd-muted/30 px-6 py-24">
      <div className="mx-auto max-w-[1200px]">
        <h2 className="text-[40px] font-semibold leading-[1.1] tracking-[-0.03em] text-fd-foreground">
          One service in your stack.
        </h2>
        <p className="mt-4 max-w-[74ch] text-[17px] leading-[1.62] text-fd-muted-foreground">
          Your backend decides what to send and when. Chimely makes it durable, real-time, and
          renderable. Postgres owns correctness; Redis owns the hot path; a Redis-less mode exists
          for small single-node deployments.
        </p>

        <div className="mt-12 flex flex-col items-center justify-center gap-3 lg:flex-row">
          <Node title="Your backend" sub="decides what & when" />
          <Connector label="POST /v1/notifications" />
          <Node title="Chimely" sub="API + workers" accent />
          <Connector />
          <div className="flex w-full max-w-[260px] flex-col gap-2.5 lg:w-auto lg:max-w-[180px] lg:flex-1">
            <div className="rounded-xl border border-fd-border bg-fd-card px-4 py-3 text-center">
              <div className="text-[14px] font-semibold text-fd-foreground">Postgres</div>
              <div className="mt-0.5 text-[12px] text-fd-muted-foreground">source of truth</div>
            </div>
            <div className="rounded-xl border border-fd-border bg-fd-card px-4 py-3 text-center">
              <div className="text-[14px] font-semibold text-fd-foreground">Redis</div>
              <div className="mt-0.5 text-[12px] text-fd-muted-foreground">real-time</div>
            </div>
          </div>
          <Connector />
          <Node
            title={<code className="font-mono text-[0.92em] text-[#1264FF]">&lt;Inbox /&gt;</code>}
            sub="REST + SSE"
          />
        </div>
      </div>
    </section>
  );
}
