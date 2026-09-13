// No colour outside the tokens. Every colour literal in the stylesheets and
// components must be one of the dark tokens, the light tokens or the one
// accent. User content (images) is not chrome and is not covered here.
import { describe, it, expect } from "vitest";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, extname } from "node:path";

const DARK = ["#000000", "#0d0d0d", "#1a1a1a", "#262626", "#404040", "#808080", "#ffffff"];
const LIGHT = ["#ffffff", "#f5f5f5", "#ebebeb", "#dcdcdc", "#b3b3b3", "#6b6b6b", "#0d0d0d"];
const ACCENT = ["#7a8fa6", "#4f6479"];
const ALLOWED = new Set([...DARK, ...LIGHT, ...ACCENT]);
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
  if (h.length === 9) return h.slice(0, 7);
  if (h.length === 5) return `#${h[1]}${h[1]}${h[2]}${h[2]}${h[3]}${h[3]}`;
  return h;
};

describe("palette", () => {
  const files = [...walk(ROOT), join(__dirname, "..", "index.html")];

  it("uses only the token colours (dark, light, one accent)", () => {
    const offenders: string[] = [];
    for (const f of files) {
      const text = readFileSync(f, "utf8");
      for (const m of text.matchAll(/#[0-9a-fA-F]{3,8}\b/g)) {
        const hex = m[0];
        if (![4, 5, 7, 9].includes(hex.length)) continue;
        if (!/^#[0-9a-fA-F]+$/.test(hex)) continue;
        if (!ALLOWED.has(expand(hex))) offenders.push(`${f}: ${hex}`);
      }
    }
    expect(offenders, offenders.join("\n")).toEqual([]);
  });

  it("uses no rgb()/hsl() functions or gradients", () => {
    const offenders: string[] = [];
    for (const f of files) {
      const text = readFileSync(f, "utf8");
      for (const m of text.matchAll(/\b(rgba?|hsla?|color-mix|oklch|lab|lch)\(|linear-gradient|radial-gradient|conic-gradient|backdrop-filter/g)) {
        offenders.push(`${f}: ${m[0]}`);
      }
    }
    expect(offenders, offenders.join("\n")).toEqual([]);
  });

  it("uses no Tailwind colour utilities outside the token names", () => {
    const banned = /\b(bg|text|border|ring|fill|stroke|from|to|via|outline|shadow|decoration|accent|caret|divide)-(red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose|slate|gray|zinc|neutral|stone)-\d+/g;
    const offenders: string[] = [];
    for (const f of files) {
      const text = readFileSync(f, "utf8");
      for (const m of text.matchAll(banned)) offenders.push(`${f}: ${m[0]}`);
    }
    expect(offenders, offenders.join("\n")).toEqual([]);
  });

  it("defines the eight roles for both themes and exactly one accent", () => {
    const tokens = readFileSync(join(ROOT, "styles", "tokens.css"), "utf8");
    expect(tokens).toContain("--color-*: initial");
    const dark = tokens.slice(tokens.indexOf('html[data-theme="dark"]'), tokens.indexOf('html[data-theme="light"]'));
    const light = tokens.slice(tokens.indexOf('html[data-theme="light"]'));
    const colours = (s: string) => [...s.matchAll(/--t-[a-z0-9-]+:\s*(#[0-9a-fA-F]{6})/g)].map((m) => m[1]!.toLowerCase());
    expect(new Set(colours(dark))).toEqual(new Set([...DARK, ACCENT[0]!]));
    expect(new Set(colours(light))).toEqual(new Set([...LIGHT, ACCENT[1]!]));
    const roles = (s: string) => [...s.matchAll(/--t-([a-z0-9-]+):/g)].map((m) => m[1]);
    expect(roles(dark)).toEqual(["bg", "surface", "surface-2", "border", "accent", "muted", "fg", "brand"]);
    expect(roles(light)).toEqual(roles(dark));
  });

  it("bundles Inter locally and loads no remote font", () => {
    const css = readFileSync(join(ROOT, "styles", "app.css"), "utf8");
    expect(css).toMatch(/@font-face[\s\S]*Inter[\s\S]*assets\/fonts\/InterVariable\.woff2/);
    expect(css).not.toMatch(/https?:\/\//);
    expect(statSync(join(ROOT, "assets", "fonts", "InterVariable.woff2")).size).toBeGreaterThan(100_000);
  });
});
