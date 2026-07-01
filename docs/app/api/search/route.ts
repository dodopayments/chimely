import { createFromSource } from 'fumadocs-core/search/server';
import { source } from '@/lib/source';

// The docs ship as a static export with no server runtime, so the default
// server search endpoint never runs. staticGET emits the Orama index as a
// build-time asset. Pairs with search type "static" in app/layout.tsx.
export const dynamic = 'force-static';
export const revalidate = false;

export const { staticGET: GET } = createFromSource(source);
