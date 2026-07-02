import type { InboxItem, InboxItemId, InboxItemSource } from '@chimely/client';
import type { ReactNode } from 'react';
import { useEffect, useRef } from 'react';
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
}

/** Scrolling list with the infinite-scroll sentinel. Internal to the package. */
export function NotificationList<TPayload>(props: NotificationListProps<TPayload>): ReactNode {
  const { items, hasMore, fetchMore, markRead, onItem, cls, strings } = props;
  const listRef = useRef<HTMLUListElement | null>(null);
  const sentinelRef = useRef<HTMLLIElement | null>(null);

  // Called for the re-render only, formatTimestamp reads the clock itself.
  // The tick is mount scoped: under <Inbox /> the list exists only while the
  // popover is open, an always mounted <InboxContent /> ticks once a minute
  // for its lifetime so relative timestamps stay current.
  useNow(60_000);

  useEffect(() => {
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
  }, [hasMore, fetchMore]);

  return (
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
                onClick={() => onItem(item)}
                renderSubject={props.renderSubject}
                renderBody={props.renderBody}
                renderAvatar={props.renderAvatar}
              />
            )}
          </li>
        ))
      )}
      <li ref={sentinelRef} className="chimely-sentinel" aria-hidden="true" />
    </ul>
  );
}
