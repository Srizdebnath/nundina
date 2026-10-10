import { Button } from "@/components/ui/Button";
import { APP_URL } from "@/lib/site";

export function PreFooterCta() {
  return (
    <section className="relative border-t border-gold-500/20 bg-obsidian-950 py-24">
      <div className="mx-auto max-w-5xl px-6 text-center">
        <div className="etched-card etched-double-border relative overflow-hidden rounded-sm p-10 shadow-[0_0_60px_rgba(234,193,131,0.08)] sm:p-16">
          <div className="relative z-10">
            <h2 className="mb-4 font-cormorant text-4xl font-normal text-gold-100 sm:text-5xl lg:text-6xl">
              Enter the New Forum of Private Capital.
            </h2>
            <p className="mx-auto mb-10 max-w-xl text-sm font-light text-gold-400 sm:text-base">
              Participate in the devnet pilot — NAV 1.000000, weekly on-chain marks, Squads-governed. No
              yield promised; not audited.
            </p>
            <div className="flex flex-col items-center justify-center space-y-4 sm:flex-row sm:space-y-0 sm:space-x-6">
              <Button href={APP_URL} arrow className="hover:scale-[1.02]">
                Launch Application
              </Button>
              <Button href="/deployments" variant="outline">
                View Deployments
              </Button>
            </div>
          </div>
        </div>
      </div>
    </section>
  );
}
