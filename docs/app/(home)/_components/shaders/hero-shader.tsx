'use client';

import dynamic from 'next/dynamic';
import { useTheme } from 'next-themes';
import { useEffect, useState } from 'react';

/**
 * Hero background shaders (@paper-design/shaders-react, pinned 0.0.76).
 * WebGL/canvas + client-only: each is loaded with ssr:false so it never runs
 * during SSR/hydration. This is the eager, above-the-fold shader. Keep the page
 * total at two shader instances (this + the closing FlutedGlass) to stay under
 * the browser WebGL-context cap.
 *
 * `HeroShader` resolves the theme and renders one of two self-contained
 * variants: `DarkShader` (the near-black brand palette) or `LightShader` (a soft
 * blue-on-white palette). Keeping them separate means the dark path is never
 * touched when the light path changes.
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
  const [mounted, setMounted] = useState(false);
  const { resolvedTheme } = useTheme();

  useEffect(() => setMounted(true), []);

  useEffect(() => {
    const mq = window.matchMedia('(prefers-reduced-motion: reduce)');
    const update = () => setReduced(mq.matches);
    update();
    mq.addEventListener('change', update);
    return () => mq.removeEventListener('change', update);
  }, []);

  // Render nothing until the theme resolves on the client, so light mode never
  // flashes the dark palette. The static base gradient covers this window.
  if (!mounted) return null;

  const light = resolvedTheme === 'light';

  // Honor reduced motion: static on-brand gradient instead of the animated shader.
  if (reduced) return <HeroGradient light={light} />;

  return light ? <LightShader shader={shader} /> : <DarkShader shader={shader} />;
}

/** Near-black brand palette. This is the original hero shader, unchanged. */
function DarkShader({ shader }: { shader: HeroShaderName }) {
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

/** Soft blue-on-white palette, mirroring `DarkShader` for light mode. */
function LightShader({ shader }: { shader: HeroShaderName }) {
  if (shader === 'mesh') {
    return (
      <MeshGradient
        colors={['#ffffff', '#e6efff', '#c7dbff', '#7aa5f5']}
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
        colors={['#1264FF', '#3b82f6', '#7aa5f5']}
        colorBack="#ffffff"
        scale={0.4}
        speed={0.8}
        style={FILL}
      />
    );
  }
  if (shader === 'warp') {
    return <Warp colors={['#ffffff', '#dff0e8', '#9cc0ff']} speed={0.5} scale={1} style={FILL} />;
  }
  return (
    <ColorPanels
      colors={['#1467ff']}
      colorBack="#ffffff"
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
export function HeroGradient({ light = false }: { light?: boolean }) {
  return (
    <div
      aria-hidden
      className="absolute inset-0"
      style={{
        background: light
          ? 'radial-gradient(120% 90% at 80% 14%, rgba(18,100,255,0.14), transparent 58%), radial-gradient(110% 105% at 8% 96%, rgba(0,79,50,0.05), transparent 60%), #ffffff'
          : 'radial-gradient(120% 90% at 80% 14%, rgba(18,100,255,0.40), transparent 58%), radial-gradient(110% 105% at 8% 96%, rgba(0,79,50,0.55), transparent 60%), radial-gradient(90% 80% at 52% 48%, rgba(8,92,82,0.18), transparent 72%), #04070a',
      }}
    />
  );
}
