import type { ReactNode } from 'react';
import { forwardRef, useEffect, useMemo } from 'react';
import type { InboxAppearance } from '../appearance';
import { slotClass, slotStyle, variablesToStyle } from '../appearance';
import { useUnseenCount } from '../hooks';
import type { InboxLocalization } from '../localization';
import { mergeLocalization } from '../localization';
import { ensureStyles } from '../styles';
import { BellIcon } from './icons';

/**
 * Standalone bell button with the unseen badge, for custom popovers and
 * layouts. Reads the client from the enclosing ChimelyProvider. Does NOT
 * call markAllSeen: clearing the badge on open is the popover's contract,
 * so custom popovers call client.markAllSeen() themselves.
 */
export interface BellProps {
  appearance?: InboxAppearance;
  localization?: Partial<InboxLocalization>;
  /** Reflected as aria-expanded when provided. */
  open?: boolean;
  onClick?: () => void;
  /** Id of the dialog this bell controls, for aria wiring. */
  popupId?: string;
  renderBell?: (ctx: { unseenCount: number; open: boolean }) => ReactNode;
}

export const Bell = forwardRef<HTMLButtonElement, BellProps>(function Bell(props, ref) {
  const { count: unseenCount } = useUnseenCount();
  const strings = mergeLocalization(props.localization);
  const open = props.open === true;
  // Bell re-renders on every unseen tick, a fresh style object per render
  // would defeat React's shallow style comparison.
  const style = useMemo(
    () => slotStyle(props.appearance, 'bell', variablesToStyle(props.appearance?.variables)),
    [props.appearance],
  );

  useEffect(() => {
    ensureStyles();
  }, []);

  return (
    <button
      ref={ref}
      type="button"
      className={slotClass(props.appearance?.classNames, 'bell')}
      style={style}
      aria-label={strings.bellLabel}
      aria-expanded={props.open === undefined ? undefined : open}
      aria-haspopup={props.popupId === undefined ? undefined : 'dialog'}
      aria-controls={props.popupId !== undefined && open ? props.popupId : undefined}
      onClick={props.onClick}
    >
      {props.renderBell ? (
        props.renderBell({ unseenCount, open })
      ) : (
        <>
          {props.appearance?.icons?.bell ? props.appearance.icons.bell() : <BellIcon />}
          {unseenCount > 0 && (
            <span
              className={slotClass(props.appearance?.classNames, 'badge')}
              style={slotStyle(props.appearance, 'badge')}
            >
              {unseenCount > 99 ? '99+' : unseenCount}
            </span>
          )}
        </>
      )}
    </button>
  );
});
