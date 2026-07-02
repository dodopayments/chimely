import type { ChimelyError, InboxItem, InboxItemId, InboxItemSource } from '@chimely/client';
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
  /** Last client error. Non-null pauses the fill loop below. */
  error: ChimelyError | null;
  /** ARIA tabpanel linkage, set when a tab strip controls the list. */
  panel?: { id: string; labelledBy: string };
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
  const { items, hasMore, fetchMore, error, markRead, onItem, cls, strings } = props;
  const listRef = useRef<HTMLUListElement | null>(null);
  const sentinelRef = useRef<HTMLLIElement | null>(null);

  // Minute tick keeps relative timestamps current while the list is visible.
  useNow(60_000);

  // Visibility is state, not a one-shot trigger: while the sentinel stays in
  // view (a sparse tab filter, a short list) the fill effect below keeps
  // fetching until items push it out or pages run out. A tab whose filter
  // matches few items therefore pages through the whole inbox on activation.
  // Server-side per-tab counts (#39) are the planned bound.
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

  // The loop advances when fetchMore settles, not on render timing: a page
  // can land and re-render before the client clears its in-flight coalescing
  // guard, which would stall a purely render-driven drain. The error gate
  // pauses the drain after a failed fetch, otherwise the still-visible
  // sentinel would retry a failing server in a tight loop. The client clears
  // error on its next successful operation, which resumes the drain.
  const [fillTick, setFillTick] = useState(0);
  // biome-ignore lint/correctness/useExhaustiveDependencies: fillTick re-checks after each settled page
  useEffect(() => {
    if (sentinelVisible && hasMore && error === null) {
      // fetchMore coalesces concurrent calls and no-ops once exhausted.
      void fetchMore()
        .catch(() => undefined)
        .finally(() => {
          setFillTick((tick) => tick + 1);
        });
    }
  }, [sentinelVisible, hasMore, error, fetchMore, fillTick]);

  return (
    <ul
      ref={listRef}
      className={cls('list')}
      {...(props.panel && {
        id: props.panel.id,
        role: 'tabpanel',
        'aria-labelledby': props.panel.labelledBy,
      })}
    >
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
