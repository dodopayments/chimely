import type { ReactNode } from 'react';
import { BellIcon, BoltIcon, ScopeIcon, SendIcon, ServerIcon, ShieldIcon } from './icons';

const Mono = ({ children, accent }: { children: ReactNode; accent?: boolean }) => (
  <code
    className={
      accent
        ? 'font-mono text-[0.88em] text-[#1264FF]'
        : 'font-mono text-[0.9em] text-fd-foreground'
    }
  >
    {children}
  </code>
);

type Feature = { id: string; icon: ReactNode; title: ReactNode; body: ReactNode };

const features: Feature[] = [
  {
    id: 'inbox',
    icon: <BellIcon className="size-[19px]" />,
    title: (
      <>
        Drop-in <Mono accent>&lt;Inbox /&gt;</Mono>
      </>
    ),
    body: (
      <>
        Bell, unread badge, popover list, infinite scroll, per-category preferences. Themed with CSS
        variables, overridable with render props, zero styling dependencies. Or go headless with{' '}
        <Mono>useNotifications</Mono> and <Mono>useUnreadCount</Mono>.
      </>
    ),
  },
  {
    id: 'notify',
    icon: <SendIcon className="size-[19px]" />,
    title: 'One call to notify',
    body: (
      <>
        <Mono>POST /v1/notifications</Mono> with a subscriber, a category, and a payload.{' '}
        <Mono>deliver_at</Mono> schedules it. No workflows, no steps, no triggers-as-indirection.
      </>
    ),
  },
  {
    id: 'self-host',
    icon: <ServerIcon className="size-[19px]" />,
    title: 'Self-host everything',
    body: "One binary plus Postgres and Redis, running next to your app, your users' data on your own infra. Server is AGPL-3.0; the SDKs are MIT.",
  },
  {
    id: 'realtime',
    icon: <BoltIcon className="size-[19px]" />,
    title: 'Real-time, and correct',
    body: 'Live updates over SSE, with REST as the source of truth. Postgres is authoritative; Redis can fall over and you lose hints, never notifications.',
  },
  {
    id: 'durable',
    icon: <ShieldIcon className="size-[19px]" />,
    title: 'Built not to lose things',
    body: 'Transactional outbox, idempotent at-least-once workers, an idempotency key on every send, and an append-only status timeline that answers “did it send?”',
  },
  {
    id: 'small',
    icon: <ScopeIcon className="size-[19px]" />,
    title: 'Deliberately small',
    body: 'No workflow engine, no server-side templates, no email or SMS. Web-push lands later as another transport for the same notification, not a migration.',
  },
];

export function FeatureGrid() {
  return (
    <section className="border-t border-fd-border bg-fd-background px-6 py-24">
      <div className="mx-auto max-w-[1200px]">
        <h2 className="chimely-display max-w-[20ch] text-balance text-[clamp(2rem,4vw,2.875rem)] leading-[1.08] tracking-[-0.005em] text-fd-foreground">
          {'Everything an inbox needs. '}
          <span className="italic text-[#1264FF]">Nothing</span>
          {" it doesn't."}
        </h2>
        <div className="mt-12 grid gap-[18px] [grid-template-columns:repeat(auto-fit,minmax(300px,1fr))]">
          {features.map((feature) => (
            <div
              key={feature.id}
              className="flex flex-col gap-3.5 rounded-2xl border border-fd-border bg-fd-card px-6 py-[26px]"
            >
              <div className="grid size-[38px] place-items-center rounded-[10px] bg-[#1264FF]/[0.14] text-[#1264FF]">
                {feature.icon}
              </div>
              <h3 className="text-[17px] font-semibold text-fd-foreground">{feature.title}</h3>
              <p className="text-[14.5px] leading-[1.62] text-fd-muted-foreground">
                {feature.body}
              </p>
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}
