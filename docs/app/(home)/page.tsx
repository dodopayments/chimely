import { Architecture } from './_components/architecture';
import { ClosingCTA } from './_components/closing-cta';
import { Comparison } from './_components/comparison';
import { FeatureGrid } from './_components/feature-grid';
import { Hero } from './_components/hero';
import { Quickstart } from './_components/quickstart';
import { SiteFooter } from './_components/site-footer';
import { SiteHeader } from './_components/site-header';
import { geistMono } from './fonts';

export default function HomePage() {
  return (
    <main className={`${geistMono.variable} bg-fd-background text-fd-foreground`}>
      <SiteHeader />

      {/*
        Treatments, selected via props:
          <Hero layout="centered" | "split" | "left" | "focus"
                shader="panels" | "mesh" | "dots" | "warp" />
          <ClosingCTA variant="centered" | "inset" />
      */}
      <Hero layout="left" shader="mesh" />
      <FeatureGrid />
      <Architecture />
      <Comparison />
      <Quickstart />
      <ClosingCTA variant="inset" />

      <SiteFooter />
    </main>
  );
}
