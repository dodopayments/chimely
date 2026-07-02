import type { InboxFilterView, InboxItem, WellKnownPayload } from '@chimely/client';
import type { ReactNode } from 'react';
import { useEffect, useMemo, useState } from 'react';
import type { InboxAppearance, InboxSlot } from '../appearance';
import { slotClass, slotStyle, variablesToStyle } from '../appearance';
import { useNotifications } from '../hooks';
import type { InboxLocalization } from '../localization';
import { mergeLocalization } from '../localization';
import { navigation, resolveActionUrl } from '../navigation';
import { ensureStyles } from '../styles';
import type { ItemRenderProps } from './DefaultItem';
import { GearIcon } from './icons';
import { NotificationList } from './NotificationList';
import { Preferences } from './Preferences';

/**
 * One tab of the inbox list. The filter runs client-side over loaded items;
 * omitting it shows everything. Unread counts per tab cover loaded pages
 * only.
 */
export interface InboxTab<TPayload = WellKnownPayload> {
  label: string;
  icon?: ReactNode;
  filter?: (item: InboxItem<TPayload>) => boolean;
}

/**
 * The popover interior without the bell or the popover shell: header,
 * notification list, preferences view, and footer. For custom popovers,
 * drawers, and full-page inboxes inside a ChimelyProvider. Custom containers
 * own the open state and call client.markAllSeen() when they open.
 */
export interface InboxContentProps<TPayload = WellKnownPayload> extends ItemRenderProps<TPayload> {
  /** Tab strip between header and list. Omit for the untabbed inbox. */
  tabs?: ReadonlyArray<InboxTab<TPayload>>;
  appearance?: InboxAppearance;
  localization?: Partial<InboxLocalization>;
  /** Show the per-category preferences panel. Default: true. */
  preferencesPanel?: boolean;
  /**
   * Item click handler. Default behavior (markRead + follow
   * `payload.action_url` if present) runs unless this returns false.
   */
  // biome-ignore lint/suspicious/noConfusingVoidType: frozen contract type
  onItemClick?: (item: InboxItem<TPayload>) => boolean | void;
  /**
   * SPA navigation for same-origin action URLs, called with the path form
   * (pathname + search + hash). Cross-origin URLs still use full navigation.
   */
  routerPush?: (url: string) => void;
  renderItem?: (ctx: { item: InboxItem<TPayload>; markRead: () => Promise<void> }) => ReactNode;
  renderEmpty?: () => ReactNode;
  renderFooter?: () => ReactNode;
}

export function InboxContent<TPayload = WellKnownPayload>(
  props: InboxContentProps<TPayload>,
): ReactNode {
  const {
    items,
    isLoading,
    hasMore,
    lastRefreshNewItemIds,
    filter,
    fetchMore,
    markRead,
    markUnread,
    markAllRead,
    setFilter,
  } = useNotifications<TPayload>();
  const [showPreferences, setShowPreferences] = useState(false);
  const [activeTabIndex, setActiveTabIndex] = useState(0);

  const tabs = props.tabs;
  const hasTabs = tabs !== undefined && tabs.length > 0;
  const activeIndex = hasTabs ? Math.min(activeTabIndex, tabs.length - 1) : 0;
  const activeFilter = hasTabs ? tabs[activeIndex]?.filter : undefined;
  const visibleItems = activeFilter ? items.filter(activeFilter) : items;

  const strings = mergeLocalization(props.localization);
  const classNames = props.appearance?.classNames;
  const cls = (slot: InboxSlot): string => slotClass(classNames, slot);
  const style = useMemo(
    () => slotStyle(props.appearance, 'content', variablesToStyle(props.appearance?.variables)),
    [props.appearance],
  );

  useEffect(() => {
    ensureStyles();
  }, []);

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

  return (
    <div className={cls('content')} style={style}>
      <div className={cls('header')} style={slotStyle(props.appearance, 'header')}>
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
              <select
                className={cls('filter')}
                style={slotStyle(props.appearance, 'filter')}
                aria-label={strings.filterLabel}
                value={filter}
                onChange={(event) => {
                  void setFilter(event.target.value as InboxFilterView);
                }}
              >
                <option value="default">{strings.filterInbox}</option>
                <option value="unread">{strings.filterUnread}</option>
              </select>
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
                  {props.appearance?.icons?.gear ? props.appearance.icons.gear() : <GearIcon />}
                </button>
              )}
            </div>
          </>
        )}
      </div>
      {hasTabs && !showPreferences && (
        <div className={cls('tabs')} role="tablist" style={slotStyle(props.appearance, 'tabs')}>
          {tabs.map((tab, index) => {
            const unread = (tab.filter ? items.filter(tab.filter) : items).filter(
              (item) => !item.read,
            ).length;
            return (
              <button
                // biome-ignore lint/suspicious/noArrayIndexKey: tabs are a static configuration list
                key={`${index}-${tab.label}`}
                type="button"
                role="tab"
                aria-selected={index === activeIndex}
                className={index === activeIndex ? `${cls('tab')} ${cls('tabActive')}` : cls('tab')}
                style={
                  index === activeIndex
                    ? slotStyle(props.appearance, 'tabActive', slotStyle(props.appearance, 'tab'))
                    : slotStyle(props.appearance, 'tab')
                }
                onClick={() => setActiveTabIndex(index)}
              >
                {tab.icon}
                <span>{tab.label}</span>
                {unread > 0 && (
                  <span className="chimely-tab-count">{unread > 99 ? '99+' : unread}</span>
                )}
              </button>
            );
          })}
        </div>
      )}
      {showPreferences ? (
        <Preferences appearance={props.appearance} localization={props.localization} />
      ) : (
        <NotificationList
          // Remount on tab switch: resets scroll and the sentinel fill loop.
          key={activeIndex}
          items={visibleItems}
          hasMore={hasMore}
          fetchMore={fetchMore}
          markRead={markRead}
          markUnread={markUnread}
          onItem={handleItemClick}
          cls={cls}
          strings={strings}
          appearance={props.appearance}
          newItemIds={lastRefreshNewItemIds}
          deferEmpty={activeFilter !== undefined && hasMore}
          renderItem={props.renderItem}
          renderEmpty={props.renderEmpty}
          renderSubject={props.renderSubject}
          renderBody={props.renderBody}
          renderAvatar={props.renderAvatar}
        />
      )}
      <div
        className={cls('footer')}
        aria-busy={isLoading}
        style={slotStyle(props.appearance, 'footer')}
      >
        {props.renderFooter?.()}
      </div>
    </div>
  );
}
