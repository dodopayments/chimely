'use client';

import dynamic from 'next/dynamic';
import { useEffect, useRef, useState } from 'react';

/**
 * Second (and final) shader on the page. Keep the total at two instances to stay
 * under the browser's WebGL-context cap. Client-only (ssr: false) and mounted
 * lazily: it only initializes once the closing band nears the viewport. Under
 * prefers-reduced-motion it never mounts. In both cases the parent section's own
 * background image remains as the on-brand fallback, so contrast is preserved.
 */
const FlutedGlass = dynamic(
  () => import('@paper-design/shaders-react').then((m) => m.FlutedGlass),
  { ssr: false },
);

export function FlutedGlassBand() {
  const ref = useRef<HTMLDivElement>(null);
  const [show, setShow] = useState(false);

  useEffect(() => {
    if (window.matchMedia('(prefers-reduced-motion: reduce)').matches) return;
    const el = ref.current;
    if (!el) return;
    const io = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) {
          setShow(true);
          io.disconnect();
        }
      },
      { rootMargin: '300px' },
    );
    io.observe(el);
    return () => io.disconnect();
  }, []);

  return (
    <div ref={ref} aria-hidden className="absolute inset-0">
      {show && (
        <FlutedGlass
          image="/chimely/closing-abstract.png"
          colorBack="#00000000"
          colorShadow="#000000"
          colorHighlight="#ffffff"
          size={0.5}
          shadows={0.25}
          highlights={0.1}
          shape="lines"
          angle={0}
          distortionShape="prism"
          distortion={0.5}
          shift={0}
          stretch={0}
          blur={0}
          edges={0.25}
          margin={0}
          fit="cover"
          style={{ position: 'absolute', inset: 0, width: '100%', height: '100%' }}
        />
      )}
    </div>
  );
}
