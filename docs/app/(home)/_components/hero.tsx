import { CodeProof } from './code-proof';
import { ArrowRight, GitHubIcon } from './icons';
import { links } from './links';
import { HeroGradient, HeroShader, type HeroShaderName } from './shaders/hero-shader';

const SUBHEAD =
  'The bell, the unread badge, the dropdown list — drop them into your React app with one component, and send notifications from your backend with one API call. Open-source infrastructure you run yourself: a single Rust binary, Postgres, and Redis. No workflow engine, no templates — just the inbox.';

function Kicker() {
  return (
    <span className="inline-flex w-fit items-center gap-2.5 rounded-full border border-white/[0.14] bg-white/[0.04] px-3.5 py-1.5 font-mono text-[12.5px] tracking-wide text-white/70">
      <span className="size-[7px] rounded-full bg-[#00D87D] shadow-[0_0_0_3px_rgba(0,216,125,0.18)] motion-safe:animate-pulse" />
      Open source · Self-hostable · v0.1.0
    </span>
  );
}

function Ctas() {
  return (
    <>
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
        className="inline-flex h-[46px] items-center gap-2 rounded-xl border border-white/[0.18] bg-white/[0.05] px-5 text-[15px] font-semibold text-white no-underline transition-colors hover:bg-white/10"
      >
        <GitHubIcon className="size-[17px]" /> Star on GitHub
      </a>
    </>
  );
}

function InlineCommand() {
  // `npx chimely dev` is not shipped yet — surfaced as "coming soon" (no copy
  // affordance) until the CLI lands. Docker Compose is the live path today.
  return (
    <div className="inline-flex w-fit items-center gap-3 rounded-xl border border-white/[0.13] bg-black/35 py-2.5 pl-4 pr-3.5 font-mono text-sm">
      <span className="text-[#7EE0A6]">$</span>
      <span className="whitespace-nowrap text-[#E8EAED]/70">npx chimely dev</span>
      <span className="rounded-md border border-white/15 bg-white/[0.06] px-2 py-0.5 text-[11px] font-medium tracking-wide text-white/60">
        Coming soon
      </span>
    </div>
  );
}

/**
 * Hero. `layout` selects one of four treatments:
 *  - "centered": stacked + centered, code proof full-width beneath the CTAs
 *  - "split":    text left, code proof right (stacks on mobile)
 *  - "left":     left-aligned single column, code proof full-width beneath
 *  - "focus":    oversized centered headline, no code proof
 */
export type HeroLayout = 'centered' | 'split' | 'left' | 'focus';

export function Hero({
  layout = 'centered',
  shader = 'panels',
}: {
  layout?: HeroLayout;
  shader?: HeroShaderName;
}) {
  return (
    <section className="relative overflow-hidden bg-[#05080a] text-white">
      {/* Static base gradient (shows while the shader chunk loads) */}
      <HeroGradient />
      {/* Animated background shader (client, ssr:false) layered on top */}
      <HeroShader shader={shader} />
      {/* Dark scrim so foreground text keeps contrast over the shader */}
      <div className="absolute inset-0 [background:linear-gradient(180deg,rgba(5,8,10,0.30)_0%,rgba(5,8,10,0.40)_50%,rgba(5,8,10,0.86)_100%)]" />
      {/* Faint technical grid */}
      <div className="absolute inset-0 [background-image:linear-gradient(to_right,rgba(255,255,255,0.045)_1px,transparent_1px),linear-gradient(to_bottom,rgba(255,255,255,0.045)_1px,transparent_1px)] [background-size:46px_46px] [mask-image:radial-gradient(120%_100%_at_50%_0%,#000_35%,transparent_78%)] [-webkit-mask-image:radial-gradient(120%_100%_at_50%_0%,#000_35%,transparent_78%)]" />

      <div className="relative mx-auto max-w-[1200px] px-6 pb-24 pt-32 md:pt-36">
        <HeroContent layout={layout} />
      </div>
    </section>
  );
}

function HeroContent({ layout }: { layout: HeroLayout }) {
  if (layout === 'split') {
    return (
      <div className="grid grid-cols-1 items-center gap-12 lg:grid-cols-2 lg:gap-[52px]">
        <div className="flex flex-col items-center gap-5 text-center lg:items-start lg:text-left">
          <Kicker />
          <h1 className="text-balance text-[clamp(2.1rem,5vw,3.5rem)] font-semibold leading-[1.05] tracking-[-0.035em] text-white">
            Give your app a notification inbox.
          </h1>
          <p className="max-w-[54ch] text-[18px] leading-[1.62] text-white/70">{SUBHEAD}</p>
          <div className="flex flex-wrap justify-center gap-3 lg:justify-start">
            <Ctas />
          </div>
          <InlineCommand />
        </div>
        <div className="min-w-0">
          <CodeProof />
        </div>
      </div>
    );
  }

  if (layout === 'left') {
    return (
      <div className="flex flex-col items-start gap-6 text-left">
        <Kicker />
        <h1 className="max-w-[18ch] text-balance text-[clamp(2.3rem,5.4vw,3.75rem)] font-semibold leading-[1.04] tracking-[-0.035em] text-white">
          Give your app a notification inbox.
        </h1>
        <p className="max-w-[60ch] text-[19px] leading-[1.62] text-white/70">{SUBHEAD}</p>
        <div className="flex flex-wrap justify-start gap-3">
          <Ctas />
        </div>
        <InlineCommand />
        <div className="mt-4 w-full max-w-[940px]">
          <CodeProof />
        </div>
      </div>
    );
  }

  if (layout === 'focus') {
    return (
      <div className="flex flex-col items-center gap-6 text-center">
        <Kicker />
        <h1 className="max-w-[15ch] text-balance text-[clamp(2.7rem,7vw,4.6rem)] font-semibold leading-[1.02] tracking-[-0.04em] text-white">
          Give your app a notification inbox.
        </h1>
        <p className="max-w-[60ch] text-[19px] leading-[1.62] text-white/70">{SUBHEAD}</p>
        <div className="mt-0.5 flex flex-wrap justify-center gap-3">
          <Ctas />
        </div>
        <InlineCommand />
      </div>
    );
  }

  // centered (default)
  return (
    <div className="flex flex-col items-center gap-6 text-center">
      <Kicker />
      <h1 className="max-w-[14ch] text-balance text-[clamp(2.45rem,6vw,4rem)] font-semibold leading-[1.04] tracking-[-0.035em] text-white">
        Give your app a notification inbox.
      </h1>
      <p className="max-w-[62ch] text-[19px] leading-[1.62] text-white/70">{SUBHEAD}</p>
      <div className="mt-0.5 flex flex-wrap justify-center gap-3">
        <Ctas />
      </div>
      <InlineCommand />
      <div className="mt-6 w-full max-w-[940px]">
        <CodeProof />
      </div>
    </div>
  );
}
