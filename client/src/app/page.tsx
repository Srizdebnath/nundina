import { Hero } from "@/components/landing/Hero";
import { TelemetryStrip } from "@/components/landing/TelemetryStrip";
import { Paradigm } from "@/components/landing/Paradigm";
import { PilotAsset } from "@/components/landing/PilotAsset";
import { Mechanism } from "@/components/landing/Mechanism";
import { Safeguards } from "@/components/landing/Safeguards";
import { Manifesto } from "@/components/landing/Manifesto";
import { PreFooterCta } from "@/components/landing/PreFooterCta";

export default function Home() {
  return (
    <>
      <Hero />
      <TelemetryStrip />
      <Paradigm />
      <PilotAsset />
      <Mechanism />
      <Safeguards />
      <Manifesto />
      <PreFooterCta />
    </>
  );
}
