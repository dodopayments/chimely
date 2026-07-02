import type { ChimelyClientConfig, InboxItem, WellKnownPayload } from '@chimely/client';
import { ChimelyClient } from '@chimely/client';
import { autoUpdate, computePosition, flip, offset, shift } from '@floating-ui/dom';
import type { CSSProperties, ReactNode } from 'react';
import { useContext, useEffect, useId, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { ChimelyContext, useChimelyClient } from './context';
import { useNotifications, usePreferences, useUnseenCount } from './hooks';
import type { InboxLocalization } from './localization';
import { mergeLocalization } from './localization';
import { navigation, resolveActionUrl } from './navigation';
import { ensureStyles } from './styles';
import { useNow } from './time';

/** Named slots for classNames overrides. This union only ever widens. */
export type InboxSlot =
  | 'root'
  | 'bell'
  | 'badge'
  | 'popover'
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
export interface InboxProps<TPayload = WellKnownPayload> {
  serverUrl?: string;
  environment?: string;
  subscriberId?: string;
  subscriberHash?: string;
  backoff?: ChimelyClientConfig['backoff'];

  appearance?: InboxAppearance;
  localization?: Partial<InboxLocalization>;
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
  /**
   * SPA navigation for same-origin action URLs, called with the path form
   * (pathname + search + hash). Cross-origin URLs still use full navigation.
   */
  routerPush?: (url: string) => void;
  /** Show the per-category preferences panel. Default: true. */
  preferencesPanel?: boolean;

  /**
   * Item click handler. Default behavior (markRead + follow
   * `payload.action_url` if present) runs unless this returns false.
   */
  // biome-ignore lint/suspicious/noConfusingVoidType: frozen contract type
  onItemClick?: (item: InboxItem<TPayload>) => boolean | void;

  renderItem?: (ctx: { item: InboxItem<TPayload>; markRead: () => Promise<void> }) => ReactNode;
  renderBell?: (ctx: { unseenCount: number; open: boolean }) => ReactNode;
  renderEmpty?: () => ReactNode;
  renderFooter?: () => ReactNode;
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
  const { items, isLoading, hasMore, fetchMore, markRead, markAllRead } =
    useNotifications<TPayload>();
  const { count: unseenCount } = useUnseenCount();
  const preferences = usePreferences();
  const [internalOpen, setInternalOpen] = useState(false);
  const [showPreferences, setShowPreferences] = useState(false);
  const rootRef = useRef<HTMLDivElement | null>(null);
  const bellRef = useRef<HTMLButtonElement | null>(null);
  const popoverRef = useRef<HTMLDivElement | null>(null);
  const listRef = useRef<HTMLUListElement | null>(null);
  const sentinelRef = useRef<HTMLLIElement | null>(null);
  const popoverId = useId();

  const strings = mergeLocalization(props.localization);
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

  const cls = (slot: InboxSlot): string => {
    const base = `chimely-${slot.replace(/[A-Z]/g, (char) => `-${char.toLowerCase()}`)}`;
    const custom = classNames?.[slot];
    return custom ? `${base} ${custom}` : base;
  };

  const rootStyle = useMemo(() => {
    const style: Record<string, string> = {};
    const variables = props.appearance?.variables;
    if (variables) {
      for (const [key, value] of Object.entries(variables)) {
        if (value !== undefined) {
          style[`--chimely-${key}`] = value;
        }
      }
    }
    return style as CSSProperties;
  }, [props.appearance]);

  useEffect(() => {
    ensureStyles();
  }, []);

  // Minute tick keeps relative timestamps current while the list is visible.
  useNow(isOpen && !showPreferences ? 60_000 : null);

  // markAllSeen fires on the rising edge of every open transition, controlled
  // or programmatic included. The badge clears and unread is untouched.
  const wasOpen = useRef(false);
  useEffect(() => {
    if (isOpen && !wasOpen.current) {
      void client.markAllSeen();
    }
    if (!isOpen && wasOpen.current) {
      setShowPreferences(false);
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
        setOpenState(false);
      }
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        setOpenState(false);
        bellRef.current?.focus();
      }
    };
    document.addEventListener('pointerdown', onPointerDown);
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('pointerdown', onPointerDown);
      document.removeEventListener('keydown', onKeyDown);
    };
  });

  useEffect(() => {
    if (!isOpen || showPreferences) {
      return undefined;
    }
    const list = listRef.current;
    const sentinel = sentinelRef.current;
    if (!list || !sentinel) {
      return undefined;
    }
    const loadMore = () => {
      if (hasMore) {
        void fetchMore();
      }
    };
    if (typeof IntersectionObserver !== 'undefined') {
      const observer = new IntersectionObserver(
        (entries) => {
          if (entries.some((entry) => entry.isIntersecting)) {
            loadMore();
          }
        },
        { root: list },
      );
      observer.observe(sentinel);
      return () => {
        observer.disconnect();
      };
    }
    const onScroll = () => {
      if (list.scrollTop + list.clientHeight >= list.scrollHeight - 32) {
        loadMore();
      }
    };
    list.addEventListener('scroll', onScroll);
    return () => {
      list.removeEventListener('scroll', onScroll);
    };
  }, [isOpen, showPreferences, hasMore, fetchMore]);

  const handleBellClick = () => {
    setOpenState(!isOpen);
  };

  const handleItemClick = (item: InboxItem<TPayload>) => {
    if (props.onItemClick?.(item) === false) {
      return;
    }
    void markRead({ id: item.id, source: item.source }).then(() => {
      const url = (item.payload as Partial<WellKnownPayload>).action_url;
      if (typeof url !== 'string' || url.length === 0) {
        return;
      }
      const resolved = resolveActionUrl(url);
      if (!resolved) {
        return;
      }
      if (resolved.kind === 'same-origin' && props.routerPush) {
        props.routerPush(resolved.path);
      } else {
        navigation.assign(url);
      }
    });
  };

  const categories = useMemo(() => {
    const set = new Set<string>();
    for (const item of items) {
      set.add(item.category);
    }
    for (const row of preferences.preferences) {
      set.add(row.category);
    }
    return [...set].sort();
  }, [items, preferences.preferences]);

  const isEnabled = (category: string): boolean =>
    !preferences.preferences.some(
      (row) => row.category === category && row.channel === 'in_app' && !row.enabled,
    );

  const popover = isOpen && (
    <div
      ref={popoverRef}
      id={popoverId}
      className={portal ? `${cls('popover')} chimely-popover-portal` : cls('popover')}
      style={rootStyle}
      role="dialog"
      aria-label={strings.inboxTitle}
    >
      <div className={cls('header')}>
        {showPreferences ? (
          <>
            <button
              type="button"
              className="chimely-header-action"
              aria-label={strings.backLabel}
              onClick={() => setShowPreferences(false)}
            >
              ←
            </button>
            <span className="chimely-header-title">{strings.preferencesTitle}</span>
          </>
        ) : (
          <>
            <span className="chimely-header-title">{strings.inboxTitle}</span>
            <div className="chimely-header-actions">
              <button
                type="button"
                className="chimely-header-action"
                onClick={() => {
                  void markAllRead();
                }}
              >
                {strings.markAllRead}
              </button>
              {props.preferencesPanel !== false && (
                <button
                  type="button"
                  className="chimely-header-action"
                  aria-label={strings.preferencesTitle}
                  title={strings.preferencesTitle}
                  onClick={() => setShowPreferences(true)}
                >
                  <GearIcon />
                </button>
              )}
            </div>
          </>
        )}
      </div>
      {showPreferences ? (
        <div className={cls('preferences')}>
          {categories.map((category) => (
            <label key={category} className="chimely-preference">
              <span>{strings.categoryLabels[category] ?? category}</span>
              <input
                type="checkbox"
                checked={isEnabled(category)}
                onChange={(event) => {
                  void preferences.setPreferences([
                    { category, channel: 'in_app', enabled: event.target.checked },
                  ]);
                }}
              />
            </label>
          ))}
        </div>
      ) : (
        <ul ref={listRef} className={cls('list')}>
          {items.length === 0 ? (
            <li className={cls('empty')}>
              {props.renderEmpty ? (
                props.renderEmpty()
              ) : (
                <>
                  <p className="chimely-empty-title">{strings.emptyTitle}</p>
                  <p className="chimely-empty-body">{strings.emptyBody}</p>
                </>
              )}
            </li>
          ) : (
            items.map((item) => (
              <li key={item.id} className="chimely-list-row">
                {props.renderItem ? (
                  props.renderItem({
                    item,
                    markRead: () => markRead({ id: item.id, source: item.source }),
                  })
                ) : (
                  <DefaultItem
                    item={item}
                    className={item.read ? cls('item') : `${cls('item')} ${cls('itemUnread')}`}
                    formatTimestamp={strings.formatTimestamp}
                    onClick={() => handleItemClick(item)}
                  />
                )}
              </li>
            ))
          )}
          <li ref={sentinelRef} className="chimely-sentinel" aria-hidden="true" />
        </ul>
      )}
      <div className={cls('footer')} aria-busy={isLoading}>
        {props.renderFooter?.()}
      </div>
    </div>
  );

  return (
    <div ref={rootRef} className={cls('root')} style={rootStyle}>
      <button
        ref={bellRef}
        type="button"
        className={cls('bell')}
        aria-label={strings.bellLabel}
        aria-expanded={isOpen}
        aria-haspopup="dialog"
        aria-controls={isOpen ? popoverId : undefined}
        onClick={handleBellClick}
      >
        {props.renderBell ? (
          props.renderBell({ unseenCount, open: isOpen })
        ) : (
          <>
            <BellIcon />
            {unseenCount > 0 && (
              <span className={cls('badge')}>{unseenCount > 99 ? '99+' : unseenCount}</span>
            )}
          </>
        )}
      </button>
      {portal && typeof document !== 'undefined' ? createPortal(popover, document.body) : popover}
    </div>
  );
}

function DefaultItem<TPayload>(props: {
  item: InboxItem<TPayload>;
  className: string;
  formatTimestamp: (iso: string) => string;
  onClick: () => void;
}): ReactNode {
  const { item, className, formatTimestamp, onClick } = props;
  const payload = item.payload as Partial<WellKnownPayload>;
  return (
    <button type="button" className={className} onClick={onClick}>
      {typeof payload.icon_url === 'string' && payload.icon_url.length > 0 && (
        <img className="chimely-item-icon" src={payload.icon_url} alt="" />
      )}
      <span className="chimely-item-text">
        <span className="chimely-item-title">
          {typeof payload.title === 'string' ? payload.title : item.category}
        </span>
        {typeof payload.body === 'string' && payload.body.length > 0 && (
          // Plain text by construction. React escaping keeps it that way.
          <span className="chimely-item-body">{payload.body}</span>
        )}
        <time
          className="chimely-item-time"
          dateTime={item.occurredAt}
          title={new Date(item.occurredAt).toLocaleString()}
        >
          {formatTimestamp(item.occurredAt)}
        </time>
      </span>
      {!item.read && <span className="chimely-item-dot" aria-hidden="true" />}
    </button>
  );
}

function BellIcon(): ReactNode {
  return (
    <svg width="20" height="20" viewBox="0 0 24 24" fill="none" aria-hidden="true">
      <path
        d="M12 3a6 6 0 0 0-6 6v3.2l-1.7 3.1a1 1 0 0 0 .9 1.5h13.6a1 1 0 0 0 .9-1.5L18 12.2V9a6 6 0 0 0-6-6Z"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinejoin="round"
      />
      <path d="M9.8 19.5a2.3 2.3 0 0 0 4.4 0" stroke="currentColor" strokeWidth="1.6" />
    </svg>
  );
}

function GearIcon(): ReactNode {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" aria-hidden="true">
      <circle cx="12" cy="12" r="3" stroke="currentColor" strokeWidth="1.6" />
      <path
        d="M19.4 13.5a7.6 7.6 0 0 0 0-3l2-1.5-2-3.5-2.4 1a7.7 7.7 0 0 0-2.6-1.5L14 2.5h-4l-.4 2.5a7.7 7.7 0 0 0-2.6 1.5l-2.4-1-2 3.5 2 1.5a7.6 7.6 0 0 0 0 3l-2 1.5 2 3.5 2.4-1a7.7 7.7 0 0 0 2.6 1.5l.4 2.5h4l.4-2.5a7.7 7.7 0 0 0 2.6-1.5l2.4 1 2-3.5Z"
        stroke="currentColor"
        strokeWidth="1.2"
      />
    </svg>
  );
}
