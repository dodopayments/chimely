import { createAPIPage } from 'fumadocs-openapi/ui';
import { openapi } from '@/lib/openapi';

/** Renders the generated API reference pages (see lib/openapi.ts). */
export const APIPage = createAPIPage(openapi);
