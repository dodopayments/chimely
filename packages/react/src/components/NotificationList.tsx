import type { InboxItem, InboxItemId, InboxItemSource } from '@chimely/client';
import type { ReactNode } from 'react';
import { useEffect, useRef, useState } from 'react';
import type { InboxAppearance, InboxSlot } from '../appearance';
import { slotStyle } from '../appearance';
import type { InboxLocalization } from '../localization';
import { useNow } from '../time';
import type { ItemRenderProps } from './DefaultItem';
import { DefaultItem } from './DefaultItem';

interface NotificationListProps<TPayload> extends ItemRenderProps<TPayload> {
  items: ReadonlyArray<InboxItem<TPayload>>;
  hasMore: boolean;
  fetchMore: () => Promise<void>;
  markRead: (item: { id: InboxItemId; source: InboxItemSource }) => Promise<void>;
  markUnread: (item: { id: InboxItemId; source: InboxItemSource }) => Promise<void>;
  archive: (item: { id: InboxItemId; source: InboxItemSource }) => Promise<void>;
  unarchive: (item: { id: InboxItemId; source: InboxItemSource }) => Promise<void>;
  onItem: (item: InboxItem<TPayload>) => void;
  cls: (slot: InboxSlot) => string;
  strings: Required<InboxLocalization>;
  appearance?: InboxAppearance;
  /** Ids the last refresh merged in, for the new-notification pill. */
  newItemIds?: ReadonlyArray<InboxItemId>;
  renderItem?: (ctx: { item: InboxItem<TPayload>; markRead: () => Promise<void> }) => ReactNode;
  renderEmpty?: () => ReactNode;
  /**
   * Suppress the empty state while more pages may still fill the view.
   * Set when a tab filter is active and hasMore is true.
   */
  deferEmpty?: boolean;
}

/** Scrolling list with the infinite-scroll sentinel. Internal to the package. */
export function NotificationList<TPayload>(props: NotificationListProps<TPayload>): ReactNode {
  const {
    items,
    hasMore,
    fetchMore,
    markRead,
    markUnread,
    archive,
    unarchive,
    onItem,
    cls,
    strings,
    newItemIds,
  } = props;
  const listRef = useRef<HTMLUListElement | null>(null);
  const sentinelRef = useRef<HTMLLIElement | null>(null);
  const pendingNewIds = useRef<Set<InboxItemId>>(new Set());
  const [pendingCount, setPendingCount] = useState(0);

  // Minute tick keeps relative timestamps current while the list is visible.
  useNow(60_000);

  // Visibility is state, not a one-shot trigger: while the sentinel stays in
  // view (a sparse tab filter, a short list) the fill effect below keeps
  // fetching until items push it out or pages run out.
  const [sentinelVisible, setSentinelVisible] = useState(false);

  useEffect(() => {
    const list = listRef.current;
    const sentinel = sentinelRef.current;
    if (!list || !sentinel) {
      return undefined;
    }
    if (typeof IntersectionObserver !== 'undefined') {
      const observer = new IntersectionObserver(
        (entries) => {
          setSentinelVisible(entries.some((entry) => entry.isIntersecting));
        },
        { root: list },
      );
      observer.observe(sentinel);
      return () => {
        observer.disconnect();
      };
    }
    const onScroll = () => {
      setSentinelVisible(list.scrollTop + list.clientHeight >= list.scrollHeight - 32);
    };
    list.addEventListener('scroll', onScroll);
    return () => {
      list.removeEventListener('scroll', onScroll);
    };
  }, []);

  // New arrivals prepend silently when the list is at the top. Scrolled
  // down, they accumulate behind the pill instead of yanking the viewport.
  useEffect(() => {
    if (newItemIds === undefined || newItemIds.length === 0) {
      return;
    }
    const list = listRef.current;
    if (!list || list.scrollTop <= 8) {
      return;
    }
    for (const id of newItemIds) {
      pendingNewIds.current.add(id);
    }
    setPendingCount(pendingNewIds.current.size);
  }, [newItemIds]);

  useEffect(() => {
    const list = listRef.current;
    if (!list) {
      return undefined;
    }
    const onScroll = () => {
      if (list.scrollTop <= 8 && pendingNewIds.current.size > 0) {
        pendingNewIds.current.clear();
        setPendingCount(0);
      }
    };
    list.addEventListener('scroll', onScroll);
    return () => {
      list.removeEventListener('scroll', onScroll);
    };
  }, []);

  const dismissPill = () => {
    const list = listRef.current;
    if (list) {
      if (typeof list.scrollTo === 'function') {
        list.scrollTo({ top: 0, behavior: 'smooth' });
      } else {
        list.scrollTop = 0;
      }
    }
    pendingNewIds.current.clear();
    setPendingCount(0);
  };

  // The loop advances on fetchMore resolution, not on render timing: a page
  // can land and re-render before the client clears its in-flight coalescing
  // guard, which would stall a purely render-driven drain.
  const [fillTick, setFillTick] = useState(0);
  // biome-ignore lint/correctness/useExhaustiveDependencies: fillTick re-checks after each resolved page
  useEffect(() => {
    if (sentinelVisible && hasMore) {
      // fetchMore coalesces concurrent calls and no-ops once exhausted.
      void fetchMore().then(() => {
        setFillTick((tick) => tick + 1);
      });
    }
  }, [sentinelVisible, hasMore, fetchMore, fillTick]);

  return (
    <div className="chimely-list-container">
      {pendingCount > 0 && (
        <button
          type="button"
          className={cls('pill')}
          style={slotStyle(props.appearance, 'pill')}
          onClick={dismissPill}
        >
          {strings.newNotifications(pendingCount)}
        </button>
      )}
      <ul ref={listRef} className={cls('list')} style={slotStyle(props.appearance, 'list')}>
        {items.length === 0
          ? props.deferEmpty !== true && (
              <li className={cls('empty')} style={slotStyle(props.appearance, 'empty')}>
                {props.renderEmpty ? (
                  props.renderEmpty()
                ) : (
                  <>
                    <p className="chimely-empty-title">{strings.emptyTitle}</p>
                    <p className="chimely-empty-body">{strings.emptyBody}</p>
                  </>
                )}
              </li>
            )
          : items.map((item) => (
              <li key={item.id} className="chimely-list-row">
                {props.renderItem ? (
                  props.renderItem({
                    item,
                    markRead: () => markRead({ id: item.id, source: item.source }),
                  })
                ) : (
                  <>
                    <DefaultItem
                      item={item}
                      className={item.read ? cls('item') : `${cls('item')} ${cls('itemUnread')}`}
                      style={
                        item.read
                          ? slotStyle(props.appearance, 'item')
                          : slotStyle(
                              props.appearance,
                              'itemUnread',
                              slotStyle(props.appearance, 'item'),
                            )
                      }
                      formatTimestamp={strings.formatTimestamp}
                      onClick={() => onItem(item)}
                      renderSubject={props.renderSubject}
                      renderBody={props.renderBody}
                      renderAvatar={props.renderAvatar}
                    />
                    {/* Sibling of the item button, not a child: a button
                        cannot nest inside a button. Revealed on row hover. */}
                    <span className="chimely-item-actions">
                      <button
                        type="button"
                        className="chimely-item-action"
                        aria-label={item.read ? strings.markUnreadAction : strings.markReadAction}
                        title={item.read ? strings.markUnreadAction : strings.markReadAction}
                        onClick={() => {
                          const flip = item.read ? markUnread : markRead;
                          void flip({ id: item.id, source: item.source });
                        }}
                      >
                        {item.read ? '\u25cf' : '\u2713'}
                      </button>
                      <button
                        type="button"
                        className="chimely-item-action"
                        aria-label={
                          item.archived === true ? strings.unarchiveAction : strings.archiveAction
                        }
                        title={
                          item.archived === true ? strings.unarchiveAction : strings.archiveAction
                        }
                        onClick={() => {
                          const flip = item.archived === true ? unarchive : archive;
                          void flip({ id: item.id, source: item.source });
                        }}
                      >
                        {item.archived === true ? '\u21a5' : '\u21a7'}
                      </button>
                    </span>
                  </>
                )}
              </li>
            ))}
        <li ref={sentinelRef} className="chimely-sentinel" aria-hidden="true" />
      </ul>
    </div>
  );
}
