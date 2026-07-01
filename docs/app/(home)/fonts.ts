import { Geist_Mono } from 'next/font/google';

/**
 * Geist Mono powers all monospace / code on the home page. `home.css` maps
 * Tailwind's `--font-mono` to this `--font-geist-mono` variable, and the
 * variable class is applied on the page's <main> (see page.tsx).
 */
export const geistMono = Geist_Mono({
  subsets: ['latin'],
  weight: ['400', '500', '600'],
  variable: '--font-geist-mono',
  display: 'swap',
});
