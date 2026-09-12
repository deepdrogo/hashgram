// No saturated colour in the UI chrome. Every colour literal in the
// stylesheets and components must be one of the seven tokens. User content
// (images, video) is not chrome and is not covered here.
import { describe, it, expect } from "vitest";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, extname } from "node:path";

const ALLOWED = new Set(["#000000", "#0d0d0d", "#1a1a1a", "#262626", "#404040", "#808080", "#ffffff"]);
const ROOT = join(__dirname, "..", "src");

function walk(dir: string, out: string[] = []): string[] {
  for (const e of readdirSync(dir)) {
    const p = join(dir, e);
    if (statSync(p).isDirectory()) walk(p, out);
    else if ([".css", ".tsx", ".ts", ".html"].includes(extname(p))) out.push(p);
  }
  return out;
}

const expand = (hex: string) => {
  const h = hex.toLowerCase();
  if (h.length === 4) return `#${h[1]}${h[1]}${h[2]}${h[2]}${h[3]}${h[3]}`;
  if (h.length === 9) return h.slice(0, 7); // #rrggbbaa → alpha is opacity, allowed
  if (h.length === 5) return `#${h[1]}${h[1]}${h[2]}${h[2]}${h[3]}${h[3]}`;
  return h;
};

describe("monochrome palette", () => {
  const files = [...walk(ROOT), join(__dirname, "..", "index.html")];

  it("uses only the seven allowed hex colours", () => {
    const offenders: string[] = [];
    for (const f of files) {
      const text = readFileSync(f, "utf8");
      for (const m of text.matchAll(/#[0-9a-fA-F]{3,8}\b/g)) {
        const hex = m[0];
        if (![4, 5, 7, 9].includes(hex.length)) continue;
        // Skip things that are not colours: hash comments in markdown-ish
        // strings are impossible in TSX, but "#hashtag" examples are words.
        if (!/^#[0-9a-fA-F]+$/.test(hex)) continue;
        if (/[g-zG-Z]/.test(hex)) continue;
        if (!ALLOWED.has(expand(hex))) offenders.push(`${f}: ${hex}`);
      }
    }
    expect(offenders, offenders.join("\n")).toEqual([]);
  });

  it("uses no rgb()/hsl()/named colour functions or gradients", () => {
    const offenders: string[] = [];
    for (const f of files) {
      const text = readFileSync(f, "utf8");
      for (const m of text.matchAll(/\b(rgba?|hsla?|color-mix|oklch|lab|lch)\(|linear-gradient|radial-gradient|conic-gradient/g)) {
        offenders.push(`${f}: ${m[0]}`);
      }
    }
    expect(offenders, offenders.join("\n")).toEqual([]);
  });

  it("uses no Tailwind colour utilities outside the token names", () => {
    // With `--color-*: initial` in tokens.css these would silently produce
    // nothing; catching them here keeps the source honest.
    const banned = /\b(bg|text|border|ring|fill|stroke|from|to|via|outline|shadow|decoration|accent|caret|divide)-(red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose|slate|gray|zinc|neutral|stone)-\d+/g;
    const offenders: string[] = [];
    for (const f of files) {
      const text = readFileSync(f, "utf8");
      for (const m of text.matchAll(banned)) offenders.push(`${f}: ${m[0]}`);
    }
    expect(offenders, offenders.join("\n")).toEqual([]);
  });

  it("defines exactly the seven tokens", () => {
    const tokens = readFileSync(join(ROOT, "styles", "tokens.css"), "utf8");
    const defined = [...tokens.matchAll(/--color-[a-z0-9-]+:\s*(#[0-9a-fA-F]{6})/g)].map((m) => m[1]!.toLowerCase());
    expect(new Set(defined)).toEqual(ALLOWED);
    expect(tokens).toContain("--color-*: initial");
  });
});
