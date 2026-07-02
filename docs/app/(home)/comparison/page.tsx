import type { Metadata } from 'next';
import { ArrowRight, CheckIcon, GitCompareIcon, GitHubIcon, MinusIcon } from '../_components/icons';
import { links } from '../_components/links';
import { FlutedGlassBand } from '../_components/shaders/fluted-glass-band';
import { SiteFooter } from '../_components/site-footer';
import { SiteHeader } from '../_components/site-header';
import { geistMono, instrumentSerif } from '../fonts';

export const metadata: Metadata = {
  title: 'Chimely vs Novu',
  description:
    'How Chimely, the in-app notification inbox, compares to Novu, the multi-channel notification platform.',
};

// Chimely-column claims are verified against server/ and packages/. Novu-column
// entries reflect Novu's public positioning and are hedged in the footnote.
const rows: { feature: string; chimely: string; novu: string }[] = [
  {
    feature: 'Core model',
    chimely: 'In-app notification inbox',
    novu: 'Multi-channel workflow engine',
  },
  {
    feature: 'Channels',
    chimely: 'In-app inbox, web-push later',
    novu: 'Email, SMS, push, chat, in-app',
  },
  {
    feature: 'Send a notification',
    chimely: 'One POST /v1/notifications',
    novu: 'Trigger a workflow of steps',
  },
  {
    feature: 'Larger fan-out',
    chimely: 'One broadcast, fanned out on read',
    novu: 'Topics and subscribers',
  },
  {
    feature: 'Templates',
    chimely: 'None, render in your own UI',
    novu: 'Server-side template editor',
  },
  { feature: 'Data stores', chimely: 'Postgres, Redis optional', novu: 'MongoDB and Redis' },
  { feature: 'Deployment', chimely: 'A single Rust binary', novu: 'Several coordinated services' },
  {
    feature: 'Real-time',
    chimely: 'SSE, with REST as the source of truth',
    novu: 'WebSocket (socket.io)',
  },
  {
    feature: 'React UI',
    chimely: 'Drop-in <Inbox /> plus headless hooks',
    novu: '<Inbox /> notification center',
  },
  {
    feature: 'Multi-tenancy',
    chimely: 'Run another instance',
    novu: 'Orgs and environments built in',
  },
  {
    feature: 'Delivery safety',
    chimely: 'Transactional outbox, at-least-once, idempotency keys',
    novu: 'Workflow execution engine',
  },
  { feature: 'Managed cloud', chimely: 'None, self-host only', novu: 'Novu Cloud available' },
  { feature: 'License', chimely: 'AGPL-3.0 server, MIT SDKs', novu: 'MIT, open-source core' },
];

function CompareHero() {
  return (
    <section className="relative overflow-hidden border-b border-fd-border bg-fd-background text-fd-foreground dark:bg-[#05080a] dark:text-white">
      {/* Dark-mode art: accent glow, faint grid, contrast scrim. Hidden in light. */}
      <div className="absolute inset-0 hidden dark:block">
        <div className="absolute inset-0 [background:radial-gradient(120%_90%_at_82%_6%,rgba(18,100,255,0.34),transparent_56%),radial-gradient(105%_105%_at_4%_100%,rgba(0,79,50,0.44),transparent_60%),#04070a]" />
        <div className="absolute inset-0 [background-image:linear-gradient(to_right,rgba(255,255,255,0.045)_1px,transparent_1px),linear-gradient(to_bottom,rgba(255,255,255,0.045)_1px,transparent_1px)] [background-size:46px_46px] [mask-image:radial-gradient(120%_100%_at_50%_0%,#000_30%,transparent_76%)] [-webkit-mask-image:radial-gradient(120%_100%_at_50%_0%,#000_30%,transparent_76%)]" />
        <div className="absolute inset-0 [background:linear-gradient(180deg,rgba(5,8,10,0.24)_0%,rgba(5,8,10,0.42)_52%,rgba(5,8,10,0.86)_100%)]" />
      </div>
      {/* Light-mode art: soft accent glow and a faint grid over the page background. */}
      <div className="absolute inset-0 dark:hidden [background:radial-gradient(120%_90%_at_82%_6%,rgba(18,100,255,0.10),transparent_56%),radial-gradient(105%_105%_at_4%_100%,rgba(0,79,50,0.05),transparent_60%)]" />
      <div className="absolute inset-0 dark:hidden [background-image:linear-gradient(to_right,rgba(13,13,13,0.035)_1px,transparent_1px),linear-gradient(to_bottom,rgba(13,13,13,0.035)_1px,transparent_1px)] [background-size:46px_46px] [mask-image:radial-gradient(120%_100%_at_50%_0%,#000_30%,transparent_76%)] [-webkit-mask-image:radial-gradient(120%_100%_at_50%_0%,#000_30%,transparent_76%)]" />

      <div className="relative mx-auto flex max-w-[920px] flex-col items-center gap-5 px-6 pb-[66px] pt-[140px] text-center">
        <span className="inline-flex items-center gap-2.5 rounded-full border border-fd-border bg-fd-muted/50 px-3.5 py-1.5 font-mono text-[12.5px] font-medium tracking-wide text-fd-muted-foreground dark:border-white/[0.14] dark:bg-white/[0.04] dark:text-white/70">
          <GitCompareIcon className="size-3.5" />
          Comparison
        </span>
        <h1 className="chimely-display text-balance text-[clamp(3rem,8vw,5.375rem)] leading-[1.0] tracking-[-0.01em] text-fd-foreground dark:text-white">
          Chimely <span className="italic text-fd-muted-foreground dark:text-white/60">vs</span>{' '}
          Novu
        </h1>
        <p className="max-w-[64ch] text-[19px] leading-[1.62] text-fd-muted-foreground dark:text-white/[0.74]">
          Both are open-source notification tools, but they solve different-sized problems. Novu is
          a multi-channel workflow platform. Chimely is just the in-app inbox: one binary, one
          Postgres, one{' '}
          <span className="font-mono text-[0.9em] text-fd-foreground dark:text-white">POST</span>.
        </p>
      </div>
    </section>
  );
}

function FeatureMatrix() {
  return (
    <section className="bg-fd-background px-6 py-[88px]">
      <div className="mx-auto max-w-[1000px]">
        <span className="inline-flex items-center gap-2 rounded-full border border-[#1264FF]/30 bg-[#1264FF]/10 px-3 py-[5px] font-mono text-xs font-medium uppercase tracking-wider text-[#1264FF]">
          Feature by feature
        </span>
        <h2 className="mt-3.5 text-balance chimely-display text-[clamp(2rem,4vw,3rem)] leading-[1.05] tracking-[-0.005em] text-fd-foreground">
          Where they <span className="italic text-[#1264FF]">actually</span> differ
        </h2>

        <div className="mt-[34px] overflow-x-auto">
          <table className="w-full min-w-[660px] border-separate border-spacing-0 text-left">
            <thead>
              <tr>
                <th className="w-[32%] px-[18px] pb-3.5 align-bottom" />
                <th className="w-[34%] rounded-t-xl border-b-2 border-[#1264FF] bg-[#1264FF]/[0.07] px-[18px] py-3.5 align-bottom">
                  <span className="inline-flex items-center gap-2 text-[16px] font-semibold text-fd-foreground">
                    <span className="size-2 rounded-full bg-[#1264FF] shadow-[0_0_0_3px_rgba(18,100,255,0.22)]" />
                    Chimely
                  </span>
                </th>
                <th className="w-[34%] px-[18px] pb-3.5 align-bottom text-[16px] font-semibold text-fd-muted-foreground">
                  Novu
                </th>
              </tr>
            </thead>
            <tbody>
              {rows.map((row, i) => {
                const last = i === rows.length - 1;
                return (
                  <tr key={row.feature}>
                    <td className="border-b border-fd-border px-[18px] py-[15px] align-top text-[14.5px] font-medium leading-[1.5] text-fd-muted-foreground">
                      {row.feature}
                    </td>
                    <td
                      className={`bg-[#1264FF]/[0.06] px-[18px] py-[15px] align-top text-[14.5px] font-medium leading-[1.5] text-fd-foreground ${
                        last ? 'rounded-b-xl' : 'border-b border-fd-border'
                      }`}
                    >
                      {row.chimely}
                    </td>
                    <td className="border-b border-fd-border px-[18px] py-[15px] align-top text-[14.5px] leading-[1.5] text-fd-muted-foreground">
                      {row.novu}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>

        <p className="mt-5 max-w-[82ch] font-mono text-[12.5px] leading-[1.6] text-fd-muted-foreground/80">
          Reflects each project&apos;s public positioning as of July 2026. Both are actively
          developed and details change. Chimely optimizes for a small in-app inbox, Novu for
          multi-channel orchestration.
        </p>
      </div>
    </section>
  );
}

const chimelyFits = [
  'You need an in-app inbox (the bell, the badge, the list) and not much else.',
  'You already run Postgres and want one more binary, not one more datastore.',
  'You would rather render notifications in your own components.',
  'You want to self-host with the fewest moving parts, per-tenant by instance.',
];

const novuFits = [
  'You need to orchestrate email, SMS, push and in-app from one place.',
  'You want a visual workflow editor and managed, server-side templates.',
  'A hosted cloud option matters, alongside self-hosting.',
  'Multiple teams share one messaging platform with orgs and environments.',
];

function WhichFits() {
  return (
    <section className="border-t border-fd-border bg-fd-muted/30 px-6 py-[88px]">
      <div className="mx-auto max-w-[1000px]">
        <h2 className="text-balance text-center chimely-display text-[clamp(2rem,4vw,3rem)] leading-[1.05] tracking-[-0.005em] text-fd-foreground">
          Which one <span className="italic text-[#1264FF]">fits</span> your problem?
        </h2>
        <div className="mt-10 grid gap-[18px] text-left [grid-template-columns:repeat(auto-fit,minmax(280px,1fr))]">
          <div className="rounded-[18px] border border-[#1264FF] bg-[#1264FF]/[0.06] p-7">
            <div className="flex items-center gap-2.5 text-[18px] font-semibold text-fd-foreground">
              <span className="size-2.5 rounded-full bg-[#1264FF] shadow-[0_0_0_3px_rgba(18,100,255,0.22)]" />
              Reach for Chimely when
            </div>
            <ul className="mt-[18px] flex flex-col gap-3">
              {chimelyFits.map((item) => (
                <li
                  key={item}
                  className="flex items-start gap-[11px] text-[15px] leading-[1.55] text-fd-muted-foreground"
                >
                  <CheckIcon className="mt-[3px] size-4 shrink-0 text-[#1264FF]" />
                  {item}
                </li>
              ))}
            </ul>
          </div>
          <div className="rounded-[18px] border border-fd-border bg-fd-card p-7">
            <div className="flex items-center gap-2.5 text-[18px] font-semibold text-fd-foreground">
              <span className="size-2.5 rounded-full bg-fd-muted-foreground/60" />
              Reach for Novu when
            </div>
            <ul className="mt-[18px] flex flex-col gap-3">
              {novuFits.map((item) => (
                <li
                  key={item}
                  className="flex items-start gap-[11px] text-[15px] leading-[1.55] text-fd-muted-foreground"
                >
                  <MinusIcon className="mt-[3px] size-4 shrink-0 text-fd-muted-foreground/70" />
                  {item}
                </li>
              ))}
            </ul>
          </div>
        </div>
      </div>
    </section>
  );
}

function CompareClosing() {
  return (
    <section className="relative overflow-hidden border-t border-fd-border bg-fd-background text-fd-foreground dark:bg-[#05080a] dark:text-white">
      {/* Base: the near-black abstract image in dark mode, a soft accent glow in light. */}
      <div
        className="absolute inset-0 hidden bg-cover bg-center dark:block"
        style={{ backgroundImage: 'url(/chimely/closing-abstract.png)' }}
      />
      <div className="absolute inset-0 dark:hidden [background:radial-gradient(100%_120%_at_85%_8%,rgba(18,100,255,0.08),transparent_55%)]" />
      {/* Fluted-glass shader (lazy, reduced-motion-safe, theme-aware), renders in both themes. */}
      <FlutedGlassBand />
      {/* Contrast scrim so the CTA text stays legible over the shader. */}
      <div className="absolute inset-0 hidden dark:block [background:linear-gradient(180deg,rgba(4,7,9,0.64),rgba(4,7,9,0.84))]" />
      <div className="absolute inset-0 dark:hidden [background:linear-gradient(180deg,rgba(255,255,255,0.20),rgba(255,255,255,0.48))]" />

      <div className="relative mx-auto flex max-w-[1000px] flex-col items-center gap-5 px-6 py-24 text-center">
        <h2 className="chimely-display max-w-[18ch] text-balance text-[clamp(2.5rem,5vw,3.625rem)] leading-[1.02] tracking-[-0.01em] text-fd-foreground dark:text-white">
          Just need the <span className="italic">inbox</span>?
        </h2>
        <p className="max-w-[60ch] text-[18px] leading-[1.6] text-fd-muted-foreground dark:text-white/[0.78]">
          Chimely is the in-app inbox, unbundled from the platform. One binary, one Postgres, one{' '}
          <span className="font-mono text-[0.9em] text-fd-foreground dark:text-white">POST</span>,
          self-hosted and yours.
        </p>
        <div className="mt-1 flex flex-wrap justify-center gap-3">
          <a
            href={links.docs}
            className="inline-flex h-[46px] items-center gap-2 rounded-xl bg-[#1264FF] px-[22px] text-[15px] font-semibold text-white no-underline shadow-[0_10px_30px_-10px_rgba(18,100,255,0.7)] transition-colors hover:bg-[#0b53e0]"
          >
            Get started <ArrowRight className="size-4" />
          </a>
          <a
            href={links.repo}
            target="_blank"
            rel="noopener noreferrer"
            className="inline-flex h-[46px] items-center gap-2 rounded-xl border border-fd-border bg-fd-muted/40 px-5 text-[15px] font-semibold text-fd-foreground no-underline transition-colors hover:bg-fd-muted dark:border-white/20 dark:bg-white/[0.06] dark:text-white dark:hover:bg-white/[0.12]"
          >
            <GitHubIcon className="size-[17px]" /> Star on GitHub
          </a>
          <a
            href="/"
            className="inline-flex h-[46px] items-center gap-1.5 rounded-xl px-4 text-[15px] font-semibold text-fd-muted-foreground no-underline transition-colors hover:text-fd-foreground dark:text-white/[0.82] dark:hover:text-white"
          >
            <ArrowRight className="size-4 rotate-180" /> Back to overview
          </a>
        </div>
      </div>
    </section>
  );
}

export default function ComparisonPage() {
  return (
    <main
      className={`${geistMono.variable} ${instrumentSerif.variable} chimely-home bg-fd-background text-fd-foreground`}
    >
      <SiteHeader />
      <CompareHero />
      <FeatureMatrix />
      <WhichFits />
      <CompareClosing />
      <SiteFooter />
    </main>
  );
}
