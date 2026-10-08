// Small line glyphs standing in for the SF Symbols the upstream sidebar uses.
export type GlyphName =
  | "appearance" | "rings" | "placement" | "spend" | "general" | "bell" | "network" | "about" | "search";

const paths: Record<GlyphName, string> = {
  appearance: "M8 2.2a5.8 5.8 0 1 0 0 11.6c1 0 1.3-.7.9-1.3-.5-.8 0-1.8 1-1.8h1.2a2.2 2.2 0 0 0 2.2-2.2C13.3 4.7 11 2.2 8 2.2ZM5.2 7.2h.01M7.4 5h.01M10.2 5.6h.01",
  rings: "M8 2.3a5.7 5.7 0 1 0 5.7 5.7M8 5.2a2.8 2.8 0 1 0 2.8 2.8",
  placement: "M2.5 3.5h11v9h-11ZM9.5 3.5v9",
  spend: "M3 12.5V8.5M6.3 12.5V4M9.7 12.5V6.5M13 12.5V3",
  general: "M2.5 4.5h11M2.5 8h11M2.5 11.5h11M10 3v3M5.5 6.5v3M9 10v3",
  bell: "M4.5 11.5V7.3a3.5 3.5 0 0 1 7 0v4.2l1 1h-9ZM6.7 13.8a1.4 1.4 0 0 0 2.6 0",
  network: "M8 2.3a5.7 5.7 0 1 0 0 11.4A5.7 5.7 0 0 0 8 2.3ZM2.4 8h11.2M8 2.3c-2 2.2-2 9.2 0 11.4M8 2.3c2 2.2 2 9.2 0 11.4",
  about: "M8 2.3a5.7 5.7 0 1 0 0 11.4A5.7 5.7 0 0 0 8 2.3ZM8 7.2v3.5M8 5h.01",
  search: "M7 2.8a4.2 4.2 0 1 0 0 8.4 4.2 4.2 0 0 0 0-8.4ZM10.2 10.2l3 3",
};

export function Glyph({ name }: { name: GlyphName }) {
  return (
    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d={paths[name]} />
    </svg>
  );
}
