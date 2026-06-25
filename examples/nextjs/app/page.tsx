'use client';

import { Inbox } from '@chimely/react';

// The dev-bootstrap defaults from the README. Override with NEXT_PUBLIC_*
// env vars when your chimely runs elsewhere.
const serverUrl = process.env.NEXT_PUBLIC_CHIMELY_URL ?? 'http://localhost:8080';
const environment = process.env.NEXT_PUBLIC_CHIMELY_ENVIRONMENT ?? 'demo';
const subscriberId = process.env.NEXT_PUBLIC_CHIMELY_SUBSCRIBER_ID ?? 'usr_demo';

const curl = `curl -X POST ${serverUrl}/v1/notifications \\
  -H 'Authorization: Bearer dev-secret-key' \\
  -H 'Content-Type: application/json' \\
  -d '{
    "subscriber_id": "${subscriberId}",
    "category": "demo.greeting",
    "payload": {
      "title": "Hello from curl",
      "body": "This arrived over the SSE hint stream.",
      "action_url": "${serverUrl}/docs"
    }
  }'`;

export default function Page() {
  return (
    <>
      <header className="topbar">
        <h1>Chimely quickstart</h1>
        {/* The whole integration: one component, three props. In production
            add subscriberHash, computed by your backend. */}
        <Inbox serverUrl={serverUrl} environment={environment} subscriberId={subscriberId} />
      </header>
      <main>
        <p>
          The bell above is <code>&lt;Inbox /&gt;</code> from <code>@chimely/react</code>, connected
          to <code>{serverUrl}</code> as subscriber <code>{subscriberId}</code> in the{' '}
          <code>{environment}</code> environment.
        </p>
        <p>Send yourself a notification and watch it arrive live:</p>
        <pre>
          <code>{curl}</code>
        </pre>
        <p>
          The badge increments without a refresh: the server publishes a hint over SSE and the
          widget refetches conditionally (a 304 when nothing changed). Opening the bell clears the
          badge; clicking the item marks it read and follows its <code>action_url</code>.
        </p>
      </main>
    </>
  );
}
