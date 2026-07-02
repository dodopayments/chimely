import type { InboxItem, WellKnownPayload } from '@chimely/client';
import type { CSSProperties, ReactNode } from 'react';

/**
 * Granular overrides for parts of the default item row. The content renders
 * inside the row button, the whole row is the click target. Returned nodes
 * must be non-interactive, no links or buttons. Nested interactive elements
 * are invalid HTML and behave inconsistently across browsers. Use renderItem
 * to own the whole row including its click wiring.
 */
export interface ItemRenderProps<TPayload> {
  renderSubject?: (ctx: { item: InboxItem<TPayload> }) => ReactNode;
  renderBody?: (ctx: { item: InboxItem<TPayload> }) => ReactNode;
  renderAvatar?: (ctx: { item: InboxItem<TPayload> }) => ReactNode;
}

export function DefaultItem<TPayload>(
  props: {
    item: InboxItem<TPayload>;
    className: string;
    style?: CSSProperties;
    formatTimestamp: (iso: string) => string;
    onClick: () => void;
  } & ItemRenderProps<TPayload>,
): ReactNode {
  const { item, className, style, formatTimestamp, onClick } = props;
  const payload = item.payload as Partial<WellKnownPayload>;
  return (
    <button type="button" className={className} style={style} onClick={onClick}>
      {props.renderAvatar
        ? props.renderAvatar({ item })
        : typeof payload.icon_url === 'string' &&
          payload.icon_url.length > 0 && (
            <img className="chimely-item-icon" src={payload.icon_url} alt="" />
          )}
      <span className="chimely-item-text">
        {props.renderSubject ? (
          props.renderSubject({ item })
        ) : (
          <span className="chimely-item-title">
            {typeof payload.title === 'string' ? payload.title : item.category}
          </span>
        )}
        {props.renderBody
          ? props.renderBody({ item })
          : typeof payload.body === 'string' &&
            payload.body.length > 0 && (
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
