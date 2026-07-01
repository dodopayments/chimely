import { ArrowRight, GitHubIcon } from './icons';
import { links } from './links';
import { FlutedGlassBand } from './shaders/fluted-glass-band';

const BODY =
  'AGPL-3.0 server, MIT SDKs. No accounts, no usage limits, no vendor in the loop. Star it, fork it, run it.';

function ClosingCtas({ stacked }: { stacked?: boolean }) {
  return (
    <div className={stacked ? 'flex flex-col gap-3' : 'flex flex-wrap justify-center gap-3'}>
      <a
        href={links.docs}
        className={`inline-flex h-[46px] items-center justify-center gap-2 rounded-xl bg-[#1264FF] ${stacked ? 'px-6' : 'px-[22px]'} text-[15px] font-semibold text-white no-underline shadow-[0_10px_30px_-10px_rgba(18,100,255,0.7)] transition-colors hover:bg-[#0b53e0]`}
      >
        Get started <ArrowRight className="size-4" />
      </a>
      <a
        href={links.repo}
        target="_blank"
        rel="noopener noreferrer"
        className={`inline-flex h-[46px] items-center justify-center gap-2 rounded-xl border border-white/20 bg-white/[0.06] ${stacked ? 'px-6' : 'px-5'} text-[15px] font-semibold text-white no-underline transition-colors hover:bg-white/[0.12]`}
      >
        <GitHubIcon className="size-[17px]" /> Star on GitHub
      </a>
    </div>
  );
}

/**
 * Closing CTA. The page's second shader band (FlutedGlass over a generated
 * abstract image). `variant` selects "centered" or a bordered "inset" panel.
 */
export function ClosingCTA({ variant = 'centered' }: { variant?: 'centered' | 'inset' }) {
  return (
    <section
      className="relative overflow-hidden border-t border-fd-border bg-[#05080a] bg-cover bg-center text-white"
      style={{ backgroundImage: 'url(/chimely/closing-abstract.png)' }}
    >
      {/* Subtle FlutedGlass over the image (lazy, reduced-motion-safe) */}
      <FlutedGlassBand />
      {/* Scrim for text contrast */}
      <div className="absolute inset-0 [background:linear-gradient(180deg,rgba(4,7,9,0.62),rgba(4,7,9,0.82))]" />

      <div className="relative mx-auto max-w-[1100px] px-6 py-[104px]">
        {variant === 'inset' ? (
          <div className="flex flex-wrap items-center justify-between gap-7 rounded-[22px] border border-white/[0.16] bg-[#080c0e]/50 p-11 backdrop-blur">
            <div className="min-w-[280px] flex-1">
              <h2 className="chimely-display text-balance text-[clamp(2.25rem,4.4vw,3.125rem)] leading-[1.05] tracking-[-0.01em] text-white">
                Open source. Self-hosted. <span className="italic">Yours</span>.
              </h2>
              <p className="mt-4 max-w-[54ch] text-[17px] leading-[1.6] text-white/[0.78]">
                {BODY}
              </p>
            </div>
            <ClosingCtas stacked />
          </div>
        ) : (
          <div className="flex flex-col items-center gap-5 text-center">
            <h2 className="chimely-display max-w-[16ch] text-balance text-[clamp(2.5rem,5vw,3.5rem)] leading-[1.04] tracking-[-0.01em] text-white">
              Open source. Self-hosted. <span className="italic">Yours</span>.
            </h2>
            <p className="max-w-[60ch] text-[18px] leading-[1.6] text-white/[0.78]">{BODY}</p>
            <ClosingCtas />
          </div>
        )}
      </div>
    </section>
  );
}
