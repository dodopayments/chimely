import { loader } from 'fumadocs-core/source';
import { docs } from '@/.source/server';
import { openapi } from '@/lib/openapi';

export const source = loader(
  {
    docs: docs.toFumadocsSource(),
    openapi: await openapi.staticSource({ baseDir: 'api' }),
  },
  {
    baseUrl: '/docs',
    plugins: [openapi.loaderPlugin()],
  },
);
