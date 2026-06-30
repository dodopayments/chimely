'use client';

import { useRouter } from 'next/navigation';
import { useEffect } from 'react';

// The site root forwards to the docs. In production the edge redirect in
// public/_redirects issues a real 302 before any asset is served. `next dev`
// does not read _redirects, and `redirect()` from next/navigation is
// unsupported under output: 'export', so this client shell keeps the same
// behavior for local development.
export default function HomePage() {
  const router = useRouter();
  useEffect(() => {
    router.replace('/docs');
  }, [router]);
  return null;
}
