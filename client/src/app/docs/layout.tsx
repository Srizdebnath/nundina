import { DocsSidebar } from "@/components/docs/DocsSidebar";

export default function DocsLayout({ children }: { children: React.ReactNode }) {
  return (
    <div className="pt-24">
      <div className="mx-auto max-w-[1400px] px-6 pb-24">
        <div className="lg:grid lg:grid-cols-[240px_minmax(0,1fr)] lg:gap-12">
          <DocsSidebar />
          <div className="min-w-0">{children}</div>
        </div>
      </div>
    </div>
  );
}
