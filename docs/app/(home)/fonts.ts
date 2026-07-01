import { Geist_Mono, Instrument_Serif } from 'next/font/google';

/**
 * Geist Mono powers all monospace / code on the home page. `home.css` maps
 * Tailwind's `--font-mono` to this `--font-geist-mono` variable inside the
 * `.chimely-home` scope, and both the variable class and `.chimely-home` sit
 * on the page's <main> (see page.tsx) so the mapping resolves in the same
 * scope where the variable is defined.
 */
export const geistMono = Geist_Mono({
  subsets: ['latin'],
  weight: ['400', '500', '600'],
  variable: '--font-geist-mono',
  display: 'swap',
});

/**
 * Instrument Serif is the display face for every headline (hero + section
 * H2s + closing). Loaded at 400 with a real italic so the emphasized word in
 * each heading renders in the true italic, not a synthesized slant. `home.css`
 * consumes `--font-instrument-serif` in the `.chimely-display` utility.
 */
export const instrumentSerif = Instrument_Serif({
  subsets: ['latin'],
  weight: '400',
  style: ['normal', 'italic'],
  variable: '--font-instrument-serif',
  display: 'swap',
});
