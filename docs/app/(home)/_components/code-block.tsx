import type { ReactNode } from 'react';
import { CopyButton } from './copy-button';

/**
 * A single dark code panel: language label + copy button + horizontally
 * scrollable <pre>. Code panels stay dark in both light and dark themes,
 * which is the common convention and keeps the syntax palette consistent.
 *
 * `code` is the raw text used for copy; `children` is the (optionally
 * token-highlighted) display markup.
 */
export function CodeBlock({
  lang = 'bash',
  code,
  copyLabel,
  badge,
  dot,
  elevated,
  children,
}: {
  lang?: string;
  code: string;
  copyLabel: string;
  badge?: ReactNode;
  dot?: boolean;
  elevated?: boolean;
  children: ReactNode;
}) {
  return (
    <div
      className={`chimely-code overflow-hidden rounded-[14px] border border-white/10${
        elevated ? ' shadow-[0_14px_40px_-20px_rgba(0,0,0,0.8)]' : ''
      }`}
    >
      <div className="flex items-center justify-between border-b border-white/[0.06] bg-[#0E1117] px-3.5 py-2">
        <span className="inline-flex items-center gap-2">
          {dot ? <span className="size-[7px] shrink-0 rounded-full bg-[#1264FF]" /> : null}
          <span className="font-mono text-xs font-medium text-[#8B949E]">{lang}</span>
          {badge}
        </span>
        <CopyButton text={code} label={copyLabel} />
      </div>
      {/* whitespace-pre + overflow-x-auto => code scrolls horizontally, never wraps */}
      <div className="overflow-x-auto px-[18px] py-4">
        <pre className="whitespace-pre font-mono text-sm leading-[1.7] text-[#C9D1D9]">
          {children}
        </pre>
      </div>
    </div>
  );
}
