import { CodeProof } from './code-proof';
import { ComingSoon } from './coming-soon';
import { CopyButton } from './copy-button';
import { ArrowRight, GitHubIcon } from './icons';
import { links } from './links';
import { HeroGradient, HeroShader, type HeroShaderName } from './shaders/hero-shader';

const SUBHEAD =
  'Drop in one React component, send with one API call from your backend. Open-source infrastructure you run yourself.';

function Kicker() {
  return (
    <span className="inline-flex w-fit items-center gap-2.5 rounded-full border border-white/[0.14] bg-white/[0.04] px-3.5 py-1.5 font-mono text-[12.5px] font-medium tracking-wide text-white/70">
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

/**
 * The hero headline. Instrument Serif display face with the emphasized word in
 * true italic (same white as the rest, accent italics are reserved for the
 * section H2s). `className` carries the per-treatment size / leading / width.
 */
function Headline({ className }: { className: string }) {
  return (
    <h1 className={`chimely-display text-balance text-white ${className}`}>
      Give your app a notification <span className="italic">inbox</span>.
    </h1>
  );
}

function InlineCommand() {
  return (
    <div className="inline-flex w-fit items-center gap-3 rounded-xl border border-white/[0.13] bg-black/35 py-2.5 pl-4 pr-2.5 font-mono text-sm font-medium">
      <span className="text-[#7EE0A6]">$</span>
      <span className="whitespace-nowrap text-[#E8EAED]">npx chimely dev</span>
      <ComingSoon />
      <CopyButton
        text="npx chimely dev"
        label="Copy install command"
        iconOnly
        className="grid size-[30px] place-items-center rounded-lg border border-white/10 text-zinc-400 transition-colors hover:border-white/30 hover:text-white"
      />
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
          <Headline className="text-[clamp(2.75rem,5.4vw,4.125rem)] leading-[1.03] tracking-[-0.01em]" />
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
        <Headline className="max-w-[18ch] text-[clamp(2.875rem,5.6vw,4.375rem)] leading-[1.02] tracking-[-0.01em]" />
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
        <Headline className="max-w-[15ch] text-[clamp(3rem,7vw,5.5rem)] leading-[1] tracking-[-0.01em]" />
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
      <Headline className="max-w-[14ch] text-[clamp(2.875rem,6vw,4.75rem)] leading-[1.02] tracking-[-0.01em]" />
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
