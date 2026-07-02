import type { CSSProperties, ReactNode } from 'react';

/** Named slots for classNames overrides. This union only ever widens. */
export type InboxSlot =
  | 'root'
  | 'bell'
  | 'badge'
  | 'popover'
  | 'content'
  | 'header'
  | 'tabs'
  | 'tab'
  | 'tabActive'
  | 'list'
  | 'item'
  | 'itemUnread'
  | 'empty'
  | 'footer'
  | 'pill'
  | 'filter'
  | 'preferences';

/**
 * Theming without a styling dependency: CSS custom properties applied at
 * the root, plus per-slot class hooks. Variable names are part of the
 * contract.
 */
export interface InboxAppearance {
  variables?: {
    /** Primary actions, links, unread badge and dot, focus rings. Default #1264FF. */
    colorPrimary?: string;
    /** Hover/section accent. Default #004F32. */
    colorPrimaryHover?: string;
    colorBackground?: string;
    colorForeground?: string;
    colorMuted?: string;
    /** Unread badge background. Default #1264FF. */
    colorBadge?: string;
    borderRadius?: string;
    fontFamily?: string;
    fontSize?: string;
    /** Badge, tab count, and pill text. Default #ffffff. */
    colorBadgeForeground?: string;
    /** Popover and pill box-shadow. Default 0 8px 24px rgba(0, 0, 0, 0.12). */
    shadow?: string;
    /** Extension point: forwarded as `--chimely-<key>` verbatim. */
    [customProperty: string]: string | undefined;
  };
  classNames?: Partial<Record<InboxSlot, string>>;
  /** Inline styles per slot, applied after the default classes. */
  styles?: Partial<Record<InboxSlot, CSSProperties>>;
  /** Replace the built-in SVG icons. renderBell wins over icons.bell. */
  icons?: {
    bell?: () => ReactNode;
    gear?: () => ReactNode;
  };
}

export function slotClass(classNames: InboxAppearance['classNames'], slot: InboxSlot): string {
  const base = `chimely-${slot.replace(/[A-Z]/g, (char) => `-${char.toLowerCase()}`)}`;
  const custom = classNames?.[slot];
  return custom ? `${base} ${custom}` : base;
}

export function variablesToStyle(variables: InboxAppearance['variables']): CSSProperties {
  const style: Record<string, string> = {};
  if (variables) {
    for (const [key, value] of Object.entries(variables)) {
      if (value !== undefined) {
        style[`--chimely-${key}`] = value;
      }
    }
  }
  return style as CSSProperties;
}

/** Per-slot inline style override, merged after base inline styles. */
export function slotStyle(
  appearance: InboxAppearance | undefined,
  slot: InboxSlot,
  base?: CSSProperties,
): CSSProperties | undefined {
  const override = appearance?.styles?.[slot];
  if (base === undefined) {
    return override;
  }
  return override === undefined ? base : { ...base, ...override };
}

/**
 * Dark preset for `appearance.variables`. Spread it and override freely:
 * `appearance={{ variables: { ...darkTheme, colorPrimary: brand } }}`.
 */
export const darkTheme: NonNullable<InboxAppearance['variables']> = {
  colorBackground: '#111827',
  colorForeground: '#e5e7eb',
  colorMuted: '#1f2937',
  colorPrimary: '#5c8dff',
  colorPrimaryHover: '#8fb0ff',
  colorBadge: '#5c8dff',
  colorBadgeForeground: '#0b1220',
  shadow: '0 8px 24px rgba(0, 0, 0, 0.5)',
};
