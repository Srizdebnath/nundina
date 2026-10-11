import { Button } from "@/components/ui/Button";
import { StarIcon } from "@/components/ui/Icons";
import { SiteHeader } from "@/components/layout/SiteHeader";
import { SiteFooter } from "@/components/layout/SiteFooter";

export default function NotFound() {
  return (
    <>
    <SiteHeader />
    <main>
    <section className="stipple-overlay relative flex min-h-[80vh] flex-col items-center justify-center bg-obsidian-950 px-6 pt-32 pb-24 text-center">
      <div className="mb-6 inline-flex h-12 w-12 items-center justify-center rounded-full border border-gold-400/40 bg-obsidian-900/80 shadow-[0_0_20px_rgba(234,193,131,0.2)]">
        <StarIcon className="h-6 w-6 text-gold-300" />
      </div>
      <span className="mb-3 font-cinzel text-xs font-semibold tracking-widest3 text-gold-400 uppercase">
        Error 404
      </span>
      <h1 className="mb-4 font-cormorant text-5xl font-normal text-gold-100 sm:text-6xl">
        This Page Is Still Being Engraved.
      </h1>
      <p className="mb-10 max-w-md text-sm font-light text-gold-400">
        No page lives at this address. Return to the forum, or open the app.
      </p>
      <Button href="/" arrow>
        Back to Home
      </Button>
    </section>
    </main>
    <SiteFooter />
    </>
  );
}
