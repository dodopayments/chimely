import type { SVGProps } from 'react';

type IconProps = SVGProps<SVGSVGElement>;

/** Stroke icons (feather-style). currentColor + 1.9 stroke by default. */
function Stroke({ children, ...props }: IconProps) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.9}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      {...props}
    >
      {children}
    </svg>
  );
}

export const GitHubIcon = (props: IconProps) => (
  <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true" {...props}>
    <path d="M12 .5C5.7.5.5 5.7.5 12c0 5.1 3.3 9.4 7.9 10.9.6.1.8-.2.8-.5v-2c-3.2.7-3.9-1.4-3.9-1.4-.5-1.3-1.3-1.7-1.3-1.7-1.1-.7.1-.7.1-.7 1.1.1 1.7 1.2 1.7 1.2 1 1.7 2.7 1.2 3.3.9.1-.7.4-1.2.7-1.5-2.5-.3-5.2-1.3-5.2-5.6 0-1.2.4-2.3 1.1-3.1-.1-.3-.5-1.4.1-3 0 0 .9-.3 3 1.2a10.5 10.5 0 0 1 5.5 0c2.1-1.5 3-1.2 3-1.2.6 1.6.2 2.7.1 3 .7.8 1.1 1.9 1.1 3.1 0 4.3-2.7 5.3-5.2 5.6.4.3.8 1 .8 2.1v3.1c0 .3.2.6.8.5 4.6-1.5 7.9-5.8 7.9-10.9C23.5 5.7 18.3.5 12 .5Z" />
  </svg>
);

export const SearchIcon = (props: IconProps) => (
  <Stroke strokeWidth={2} {...props}>
    <circle cx="11" cy="11" r="8" />
    <path d="m21 21-4.3-4.3" />
  </Stroke>
);

export const MoonIcon = (props: IconProps) => (
  <Stroke strokeWidth={2} {...props}>
    <path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z" />
  </Stroke>
);

export const SunIcon = (props: IconProps) => (
  <Stroke strokeWidth={2} {...props}>
    <circle cx="12" cy="12" r="4" />
    <path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4" />
  </Stroke>
);

export const ArrowRight = (props: IconProps) => (
  <Stroke strokeWidth={2.4} {...props}>
    <path d="M5 12h14M13 5l7 7-7 7" />
  </Stroke>
);

export const ArrowUpRight = (props: IconProps) => (
  <Stroke strokeWidth={2.2} {...props}>
    <path d="M7 17 17 7M9 7h8v8" />
  </Stroke>
);

export const CopyIcon = (props: IconProps) => (
  <Stroke strokeWidth={2} {...props}>
    <rect x="9" y="9" width="13" height="13" rx="2" />
    <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
  </Stroke>
);

export const CheckIcon = (props: IconProps) => (
  <Stroke strokeWidth={2.4} {...props}>
    <path d="M20 6 9 17l-5-5" />
  </Stroke>
);

export const XIcon = (props: IconProps) => (
  <Stroke strokeWidth={2} {...props}>
    <path d="M18 6 6 18M6 6l12 12" />
  </Stroke>
);

// Feature icons
export const BellIcon = (props: IconProps) => (
  <Stroke {...props}>
    <path d="M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9" />
    <path d="M10.3 21a1.94 1.94 0 0 0 3.4 0" />
  </Stroke>
);

export const SendIcon = (props: IconProps) => (
  <Stroke {...props}>
    <path d="m22 2-7 20-4-9-9-4Z" />
    <path d="M22 2 11 13" />
  </Stroke>
);

export const ServerIcon = (props: IconProps) => (
  <Stroke {...props}>
    <rect x="3" y="3" width="18" height="7" rx="1.5" />
    <rect x="3" y="14" width="18" height="7" rx="1.5" />
    <path d="M7 6.5h.01M7 17.5h.01" />
  </Stroke>
);

export const BoltIcon = (props: IconProps) => (
  <Stroke {...props}>
    <path d="M13 2 3 14h7l-1 8 10-12h-7l1-8Z" />
  </Stroke>
);

export const ShieldIcon = (props: IconProps) => (
  <Stroke {...props}>
    <path d="M12 3 4 6v5c0 5 3.4 8.5 8 10 4.6-1.5 8-5 8-10V6l-8-3Z" />
    <path d="m9 12 2 2 4-4" />
  </Stroke>
);

export const ScopeIcon = (props: IconProps) => (
  <Stroke {...props}>
    <circle cx="12" cy="12" r="9" />
    <path d="M8 12h8" />
  </Stroke>
);
