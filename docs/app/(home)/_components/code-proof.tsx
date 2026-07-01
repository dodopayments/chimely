import type { ReactNode } from 'react';
import { CodeBlock } from './code-block';

// Syntax token colors (neutral palette — deliberately distinct from the brand
// semantic colors, which are reserved for state).
const C = {
  str: '#7EE0A6',
  num: '#E5C07B',
  kw: '#C792EA',
  fn: '#62A6FF',
  key: '#79C0FF',
  punct: '#8B949E',
} as const;

const T = ({ c, children }: { c: string; children: ReactNode }) => (
  <span style={{ color: c }}>{children}</span>
);
// Each line is a block span so line breaks come from layout, not source newlines.
const Line = ({ children }: { children?: ReactNode }) => <span className="block">{children}</span>;

const CURL = `curl -X POST https://your-app.com/v1/notifications \\
  -H "Authorization: Bearer $CHIMELY_API_KEY" \\
  -d '{
    "subscriber_id": "usr_42",
    "category": "payment.failed",
    "payload": { "amount": 4200, "currency": "USD" },
    "idempotency_key": "evt_9f8a"
  }'`;

const INBOX = `import { Inbox } from "@chimely/react";

<Inbox subscriberId={user.id} subscriberHash={hash} />`;

/** The hero's two code snippets (curl + JSX) side by side, with a caption. */
export function CodeProof() {
  return (
    <div className="flex w-full min-w-0 flex-col gap-4">
      <div className="grid w-full min-w-0 gap-3.5 [grid-template-columns:repeat(auto-fit,minmax(290px,1fr))]">
        <CodeBlock lang="Server · bash" code={CURL} copyLabel="Copy server snippet">
          <Line>
            <T c={C.fn}>curl</T> <T c={C.punct}>-X</T> POST{' '}
            <T c={C.str}>https://your-app.com/v1/notifications</T> <T c={C.punct}>\</T>
          </Line>
          <Line>
            {'  '}
            <T c={C.punct}>-H</T>{' '}
            <T c={C.str}>&quot;Authorization: Bearer $CHIMELY_API_KEY&quot;</T> <T c={C.punct}>\</T>
          </Line>
          <Line>
            {'  '}
            <T c={C.punct}>-d</T> <T c={C.punct}>{"'{"}</T>
          </Line>
          <Line>
            {'    '}
            <T c={C.key}>&quot;subscriber_id&quot;</T>
            <T c={C.punct}>:</T> <T c={C.str}>&quot;usr_42&quot;</T>
            <T c={C.punct}>,</T>
          </Line>
          <Line>
            {'    '}
            <T c={C.key}>&quot;category&quot;</T>
            <T c={C.punct}>:</T> <T c={C.str}>&quot;payment.failed&quot;</T>
            <T c={C.punct}>,</T>
          </Line>
          <Line>
            {'    '}
            <T c={C.key}>&quot;payload&quot;</T>
            <T c={C.punct}>:</T> <T c={C.punct}>{'{'}</T> <T c={C.key}>&quot;amount&quot;</T>
            <T c={C.punct}>:</T> <T c={C.num}>4200</T>
            <T c={C.punct}>,</T> <T c={C.key}>&quot;currency&quot;</T>
            <T c={C.punct}>:</T> <T c={C.str}>&quot;USD&quot;</T> <T c={C.punct}>{'},'}</T>
          </Line>
          <Line>
            {'    '}
            <T c={C.key}>&quot;idempotency_key&quot;</T>
            <T c={C.punct}>:</T> <T c={C.str}>&quot;evt_9f8a&quot;</T>
          </Line>
          <Line>
            {'  '}
            <T c={C.punct}>{"}'"}</T>
          </Line>
        </CodeBlock>

        <CodeBlock lang="Client · tsx" code={INBOX} copyLabel="Copy client snippet">
          <Line>
            <T c={C.kw}>import</T> <T c={C.punct}>{'{'}</T> Inbox <T c={C.punct}>{'}'}</T>{' '}
            <T c={C.kw}>from</T> <T c={C.str}>&quot;@chimely/react&quot;</T>
            <T c={C.punct}>;</T>
          </Line>
          <Line>{' '}</Line>
          <Line>
            <T c={C.punct}>{'<'}</T>
            <T c={C.fn}>Inbox</T> <T c={C.key}>subscriberId</T>
            <T c={C.punct}>{'={'}</T>user.id<T c={C.punct}>{'}'}</T> <T c={C.key}>subscriberHash</T>
            <T c={C.punct}>{'={'}</T>hash<T c={C.punct}>{'}'}</T> <T c={C.punct}>{'/>'}</T>
          </Line>
        </CodeBlock>
      </div>

      <p className="text-center text-sm leading-relaxed text-zinc-400">
        Send from your backend. Render in your frontend. That&apos;s the whole integration.
      </p>
    </div>
  );
}
