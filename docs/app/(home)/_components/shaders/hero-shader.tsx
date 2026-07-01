'use client';

import dynamic from 'next/dynamic';
import { useEffect, useState } from 'react';

/**
 * Hero background shaders (@paper-design/shaders-react, pinned 0.0.76).
 * WebGL/canvas + client-only: each is loaded with ssr:false so it never runs
 * during SSR/hydration. This is the eager, above-the-fold shader. Keep the page
 * total at two shader instances (this + the closing FlutedGlass) to stay under
 * the browser WebGL-context cap.
 */
const FILL = { position: 'absolute', inset: 0, width: '100%', height: '100%' } as const;

const ColorPanels = dynamic(
  () => import('@paper-design/shaders-react').then((m) => m.ColorPanels),
  { ssr: false },
);
const MeshGradient = dynamic(
  () => import('@paper-design/shaders-react').then((m) => m.MeshGradient),
  { ssr: false },
);
const DotOrbit = dynamic(() => import('@paper-design/shaders-react').then((m) => m.DotOrbit), {
  ssr: false,
});
const Warp = dynamic(() => import('@paper-design/shaders-react').then((m) => m.Warp), {
  ssr: false,
});

export type HeroShaderName = 'panels' | 'mesh' | 'dots' | 'warp';

export function HeroShader({ shader = 'panels' }: { shader?: HeroShaderName }) {
  const [reduced, setReduced] = useState(false);

  useEffect(() => {
    const mq = window.matchMedia('(prefers-reduced-motion: reduce)');
    const update = () => setReduced(mq.matches);
    update();
    mq.addEventListener('change', update);
    return () => mq.removeEventListener('change', update);
  }, []);

  // Honor reduced motion: static on-brand gradient instead of the animated shader.
  if (reduced) return <HeroGradient />;

  if (shader === 'mesh') {
    return (
      <MeshGradient
        colors={['#04070a', '#003a25', '#0a3f8f', '#1264FF']}
        distortion={1}
        swirl={0.65}
        speed={0.4}
        style={FILL}
      />
    );
  }
  if (shader === 'dots') {
    return (
      <DotOrbit
        colors={['#1264FF', '#1f8f5b', '#3b82f6']}
        colorBack="#04070a"
        scale={0.4}
        speed={0.8}
        style={FILL}
      />
    );
  }
  if (shader === 'warp') {
    return <Warp colors={['#04070a', '#004F32', '#1264FF']} speed={0.5} scale={1} style={FILL} />;
  }
  // panels (default), the brief's ColorPanels preset
  return (
    <ColorPanels
      colors={['#1467ff']}
      colorBack="#000f0a"
      density={2.21}
      angle1={-1}
      angle2={-1}
      length={0.56}
      edges
      blur={0.15}
      fadeIn={0}
      fadeOut={1}
      gradient={0}
      speed={2}
      scale={2.32}
      rotation={360}
      offsetX={0.38}
      offsetY={0.6}
      style={FILL}
    />
  );
}

/**
 * Static on-brand gradient. Rendered as the base layer beneath the shader (so the
 * hero looks right while the shader chunk loads) and as the reduced-motion fallback.
 */
export function HeroGradient() {
  return (
    <div
      aria-hidden
      className="absolute inset-0"
      style={{
        background:
          'radial-gradient(120% 90% at 80% 14%, rgba(18,100,255,0.40), transparent 58%), radial-gradient(110% 105% at 8% 96%, rgba(0,79,50,0.55), transparent 60%), radial-gradient(90% 80% at 52% 48%, rgba(8,92,82,0.18), transparent 72%), #04070a',
      }}
    />
  );
}
