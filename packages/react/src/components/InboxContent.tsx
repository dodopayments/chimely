import type { InboxItem, WellKnownPayload } from '@chimely/client';
import type { ReactNode } from 'react';
import { useEffect, useMemo, useState } from 'react';
import type { InboxAppearance, InboxSlot } from '../appearance';
import { slotClass, variablesToStyle } from '../appearance';
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
 * The popover interior without the bell or the popover shell: header,
 * notification list, preferences view, and footer. For custom popovers,
 * drawers, and full-page inboxes inside a ChimelyProvider. Custom containers
 * own the open state and call client.markAllSeen() when they open.
 */
export interface InboxContentProps<TPayload = WellKnownPayload> extends ItemRenderProps<TPayload> {
  appearance?: InboxAppearance;
  localization?: Partial<InboxLocalization>;
  /**
   * Id placed on the header title element so a wrapping dialog can reference
   * it via aria-labelledby. The title text tracks the visible view.
   */
  titleId?: string;
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
  const { items, isLoading, hasMore, fetchMore, markRead, markAllRead } =
    useNotifications<TPayload>();
  const [showPreferences, setShowPreferences] = useState(false);

  const strings = mergeLocalization(props.localization);
  const classNames = props.appearance?.classNames;
  const cls = (slot: InboxSlot): string => slotClass(classNames, slot);
  const style = useMemo(() => variablesToStyle(props.appearance?.variables), [props.appearance]);

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
            <span id={props.titleId} className="chimely-header-title">
              {strings.preferencesTitle}
            </span>
          </>
        ) : (
          <>
            <span id={props.titleId} className="chimely-header-title">
              {strings.inboxTitle}
            </span>
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
        <Preferences appearance={props.appearance} localization={props.localization} />
      ) : (
        <NotificationList
          items={items}
          hasMore={hasMore}
          fetchMore={fetchMore}
          markRead={markRead}
          onItem={handleItemClick}
          cls={cls}
          strings={strings}
          renderItem={props.renderItem}
          renderEmpty={props.renderEmpty}
          renderSubject={props.renderSubject}
          renderBody={props.renderBody}
          renderAvatar={props.renderAvatar}
        />
      )}
      <div className={cls('footer')} aria-busy={isLoading}>
        {props.renderFooter?.()}
      </div>
    </div>
  );
}
