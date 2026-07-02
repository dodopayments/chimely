import type { ChimelyClientConfig, WellKnownPayload } from '@chimely/client';
import { ChimelyClient } from '@chimely/client';
import { autoUpdate, computePosition, flip, offset, shift } from '@floating-ui/dom';
import type { ReactNode } from 'react';
import { useContext, useEffect, useId, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import type { InboxSlot } from './appearance';
import { slotClass, variablesToStyle } from './appearance';
import type { BellProps } from './components/Bell';
import { Bell } from './components/Bell';
import type { InboxContentProps } from './components/InboxContent';
import { InboxContent } from './components/InboxContent';
import { ChimelyContext, useChimelyClient } from './context';
import { ensureStyles } from './styles';

export type { InboxAppearance, InboxSlot } from './appearance';

/**
 * Drop-in bell + badge + popover inbox.
 *
 * Two usage modes:
 * - Standalone: pass `serverUrl`/`environment`/`subscriberId`(/`subscriberHash`)
 *   and <Inbox /> constructs and owns its client.
 * - Provided: render inside <ChimelyProvider> and pass no connection props.
 *   Connection props, when present, take precedence over the provider.
 *
 * Built-in behavior (part of the contract):
 * - Opening the popover calls markAllSeen. The badge clears and unread is untouched.
 * - The list infinite-scrolls via fetchMore.
 * - A preferences panel (per-category in_app toggles) is included. Hide it
 *   with `preferencesPanel={false}`.
 */
export interface InboxProps<TPayload = WellKnownPayload> extends InboxContentProps<TPayload> {
  serverUrl?: string;
  environment?: string;
  subscriberId?: string;
  subscriberHash?: string;
  backoff?: ChimelyClientConfig['backoff'];

  /** Popover placement relative to the bell. Default: 'bottom-end'. */
  placement?: 'bottom-start' | 'bottom-end' | 'top-start' | 'top-end';
  /** Distance in px between the bell and the popover. Default: 8. */
  placementOffset?: number;
  /**
   * Render the popover in a document.body portal with fixed positioning,
   * escaping overflow and transform ancestors. Default: false.
   */
  portal?: boolean;
  /** Controlled open state. Omit to keep the popover self-managed. */
  open?: boolean;
  /** Fires on every open/close intent, controlled or not. */
  onOpenChange?: (open: boolean) => void;

  renderBell?: BellProps['renderBell'];
}

export function Inbox<TPayload = WellKnownPayload>(props: InboxProps<TPayload>): ReactNode {
  const { serverUrl, environment, subscriberId, subscriberHash, backoff } = props;
  const contextClient = useContext(ChimelyContext);
  // Connection props are all-or-nothing and take precedence over the provider.
  const [owned] = useState(() => {
    if (serverUrl === undefined) {
      return null;
    }
    if (environment === undefined || subscriberId === undefined) {
      throw new Error('standalone <Inbox /> requires serverUrl, environment, and subscriberId');
    }
    const config: ChimelyClientConfig = { serverUrl, environment, subscriberId };
    if (subscriberHash !== undefined) {
      config.subscriberHash = subscriberHash;
    }
    if (backoff !== undefined) {
      config.backoff = backoff;
    }
    return new ChimelyClient(config);
  });
  const client = owned ?? contextClient;
  if (!client) {
    throw new Error('<Inbox /> requires connection props or an enclosing <ChimelyProvider>');
  }
  useEffect(() => {
    if (!owned) {
      return undefined;
    }
    owned.connect();
    return () => {
      owned.close();
    };
  }, [owned]);
  return (
    <ChimelyContext.Provider value={client}>
      <InboxView<TPayload> {...props} />
    </ChimelyContext.Provider>
  );
}

function InboxView<TPayload>(props: InboxProps<TPayload>): ReactNode {
  const client = useChimelyClient();
  const [internalOpen, setInternalOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement | null>(null);
  const bellRef = useRef<HTMLButtonElement | null>(null);
  const popoverRef = useRef<HTMLDivElement | null>(null);
  const popoverId = useId();
  const titleId = useId();

  const placement = props.placement ?? 'bottom-end';
  const placementOffset = props.placementOffset ?? 8;
  const portal = props.portal === true;
  const classNames = props.appearance?.classNames;

  const isOpen = props.open ?? internalOpen;
  const setOpenState = (next: boolean): void => {
    if (props.open === undefined) {
      setInternalOpen(next);
    }
    props.onOpenChange?.(next);
  };
  // Kept in a ref so the dismissal effect registers its document listeners
  // once per open transition yet calls the latest onOpenChange closure.
  const setOpenStateRef = useRef(setOpenState);
  useEffect(() => {
    setOpenStateRef.current = setOpenState;
  });

  const cls = (slot: InboxSlot): string => slotClass(classNames, slot);

  const rootStyle = useMemo(
    () => variablesToStyle(props.appearance?.variables),
    [props.appearance],
  );

  useEffect(() => {
    ensureStyles();
  }, []);

  // markAllSeen fires on the rising edge of every open transition, controlled
  // or programmatic included. The badge clears and unread is untouched.
  const wasOpen = useRef(false);
  useEffect(() => {
    if (isOpen && !wasOpen.current) {
      void client.markAllSeen();
    }
    wasOpen.current = isOpen;
  }, [isOpen, client]);

  useEffect(() => {
    if (!isOpen) {
      return undefined;
    }
    const bell = bellRef.current;
    const popover = popoverRef.current;
    if (!bell || !popover) {
      return undefined;
    }
    const update = () => {
      void computePosition(bell, popover, {
        placement,
        strategy: portal ? 'fixed' : 'absolute',
        middleware: [offset(placementOffset), flip(), shift({ padding: 8 })],
      }).then(({ x, y }) => {
        popover.style.left = `${x}px`;
        popover.style.top = `${y}px`;
      });
    };
    update();
    if (typeof ResizeObserver === 'undefined') {
      return undefined;
    }
    return autoUpdate(bell, popover, update);
  }, [isOpen, placement, placementOffset, portal]);

  useEffect(() => {
    if (!isOpen) {
      return undefined;
    }
    const onPointerDown = (event: PointerEvent) => {
      if (!(event.target instanceof Node)) {
        return;
      }
      // The popover is outside the root when portaled, so check both.
      const inRoot = rootRef.current?.contains(event.target) === true;
      const inPopover = popoverRef.current?.contains(event.target) === true;
      if (!inRoot && !inPopover) {
        setOpenStateRef.current(false);
      }
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        setOpenStateRef.current(false);
        bellRef.current?.focus();
      }
    };
    document.addEventListener('pointerdown', onPointerDown);
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('pointerdown', onPointerDown);
      document.removeEventListener('keydown', onKeyDown);
    };
  }, [isOpen]);

  const popover = isOpen && (
    <div
      ref={popoverRef}
      id={popoverId}
      className={portal ? `${cls('popover')} chimely-popover-portal` : cls('popover')}
      style={rootStyle}
      role="dialog"
      aria-labelledby={titleId}
    >
      <InboxContent<TPayload>
        tabs={props.tabs}
        appearance={props.appearance}
        localization={props.localization}
        titleId={titleId}
        preferencesPanel={props.preferencesPanel}
        onItemClick={props.onItemClick}
        routerPush={props.routerPush}
        renderItem={props.renderItem}
        renderEmpty={props.renderEmpty}
        renderFooter={props.renderFooter}
        renderSubject={props.renderSubject}
        renderBody={props.renderBody}
        renderAvatar={props.renderAvatar}
      />
    </div>
  );

  return (
    <div ref={rootRef} className={cls('root')} style={rootStyle}>
      <Bell
        ref={bellRef}
        appearance={props.appearance}
        localization={props.localization}
        open={isOpen}
        popupId={popoverId}
        onClick={() => setOpenState(!isOpen)}
        renderBell={props.renderBell}
      />
      {portal && typeof document !== 'undefined' ? createPortal(popover, document.body) : popover}
    </div>
  );
}
