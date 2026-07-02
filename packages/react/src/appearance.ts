import type { CSSProperties } from 'react';

/** Named slots for classNames overrides. This union only ever widens. */
export type InboxSlot =
  | 'root'
  | 'bell'
  | 'badge'
  | 'popover'
  | 'content'
  | 'header'
  | 'list'
  | 'item'
  | 'itemUnread'
  | 'empty'
  | 'footer'
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
    /** Extension point: forwarded as `--chimely-<key>` verbatim. */
    [customProperty: string]: string | undefined;
  };
  classNames?: Partial<Record<InboxSlot, string>>;
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
