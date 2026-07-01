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
  children,
}: {
  lang?: string;
  code: string;
  copyLabel: string;
  children: ReactNode;
}) {
  return (
    <div className="chimely-code overflow-hidden rounded-2xl border border-white/10 shadow-[0_14px_40px_-20px_rgba(0,0,0,0.8)]">
      <div className="flex items-center justify-between border-b border-white/[0.06] bg-[#0E1117] px-3.5 py-2">
        <span className="font-mono text-xs tracking-tight text-zinc-500">{lang}</span>
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
