import type { InboxItem, InboxItemId, InboxItemSource } from '@chimely/client';
import type { ReactNode } from 'react';
import { useEffect, useRef, useState } from 'react';
import type { InboxSlot } from '../appearance';
import type { InboxLocalization } from '../localization';
import { useNow } from '../time';
import type { ItemRenderProps } from './DefaultItem';
import { DefaultItem } from './DefaultItem';

interface NotificationListProps<TPayload> extends ItemRenderProps<TPayload> {
  items: ReadonlyArray<InboxItem<TPayload>>;
  hasMore: boolean;
  fetchMore: () => Promise<void>;
  markRead: (item: { id: InboxItemId; source: InboxItemSource }) => Promise<void>;
  onItem: (item: InboxItem<TPayload>) => void;
  cls: (slot: InboxSlot) => string;
  strings: Required<InboxLocalization>;
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
  const { items, hasMore, fetchMore, markRead, onItem, cls, strings } = props;
  const listRef = useRef<HTMLUListElement | null>(null);
  const sentinelRef = useRef<HTMLLIElement | null>(null);

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
    <ul ref={listRef} className={cls('list')}>
      {items.length === 0
        ? props.deferEmpty !== true && (
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
          )
        : items.map((item) => (
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
                  onClick={() => onItem(item)}
                  renderSubject={props.renderSubject}
                  renderBody={props.renderBody}
                  renderAvatar={props.renderAvatar}
                />
              )}
            </li>
          ))}
      <li ref={sentinelRef} className="chimely-sentinel" aria-hidden="true" />
    </ul>
  );
}
