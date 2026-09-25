const PHASES: { id: string; label: string; status: string }[] = [
  { id: "P0", label: "Scaffold", status: "in progress" },
  { id: "P1", label: "Gate + Asset (NPCS)", status: "next" },
  { id: "P2", label: "Marks + Treasury", status: "queued" },
  { id: "P3", label: "Auction", status: "queued" },
  { id: "P4", label: "Claims + Facility", status: "queued" },
  { id: "P5", label: "Tranching", status: "queued" },
];

export default function Home() {
  return (
    <main style={{ maxWidth: 880, margin: "0 auto", padding: "48px 24px" }}>
      <p style={{ color: "#f0b429", letterSpacing: 2, fontSize: 12, margin: 0 }}>
        PILOT-DEMO-ASSET — NOT AN OFFER — SYNTHETIC LABEL
      </p>
      <h1 style={{ fontSize: 28, fontWeight: 600, margin: "12px 0 4px" }}>
        NUNDINA Pilot
      </h1>
      <p style={{ color: "#8b95a7", margin: "0 0 32px", fontSize: 14 }}>
        The pricing, queue &amp; risk-transfer engine for gated, NAV-priced
        private credit. UI completion lands in P2/P6 per SRS 08 §12.
      </p>
      <table style={{ width: "100%", borderCollapse: "collapse", fontSize: 14 }}>
        <tbody>
          {PHASES.map((p) => (
            <tr key={p.id} style={{ borderTop: "1px solid #1c2333" }}>
              <td style={{ padding: "10px 8px", color: "#5b87c7", width: 48 }}>{p.id}</td>
              <td style={{ padding: "10px 8px" }}>{p.label}</td>
              <td style={{ padding: "10px 8px", color: "#8b95a7", textAlign: "right" }}>
                {p.status}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </main>
  );
}
