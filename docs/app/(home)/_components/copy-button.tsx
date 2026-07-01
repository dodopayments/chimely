'use client';

import { useState } from 'react';
import { CheckIcon, CopyIcon } from './icons';

/** Copy-to-clipboard button with an accessible label and a transient "Copied" state. */
export function CopyButton({
  text,
  label,
  className,
  iconOnly = false,
}: {
  text: string;
  label: string;
  className?: string;
  iconOnly?: boolean;
}) {
  const [copied, setCopied] = useState(false);

  return (
    <button
      type="button"
      aria-label={label}
      onClick={() => {
        navigator.clipboard?.writeText(text);
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1600);
      }}
      className={
        className ??
        'inline-flex items-center gap-1.5 rounded-lg border border-white/10 px-2.5 py-1 text-xs font-medium text-zinc-400 transition-colors hover:border-white/25 hover:text-white'
      }
    >
      {copied ? (
        <span className="inline-flex items-center gap-1.5 text-[#00D87D]">
          <CheckIcon className="size-3.5" />
          {!iconOnly && 'Copied'}
        </span>
      ) : (
        <span className="inline-flex items-center gap-1.5">
          <CopyIcon className="size-3.5" />
          {!iconOnly && 'Copy'}
        </span>
      )}
    </button>
  );
}
