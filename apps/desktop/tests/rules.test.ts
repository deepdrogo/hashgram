// Rules from docs/PROMPT_DESKTOP_AI.md that a grep can enforce.
import { describe, it, expect } from "vitest";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, extname, relative } from "node:path";

const APPS = join(__dirname, "..", "..");
const SKIP_DIRS = new Set(["node_modules", "dist", "target", "gen", ".pnpm", "icons"]);
const TEXT_EXT = new Set([".ts", ".tsx", ".rs", ".md", ".json", ".css", ".html", ".toml", ".nsh", ".ps1", ".mjs", ".yaml", ".txt"]);

function walk(dir: string, out: string[] = []): string[] {
  for (const e of readdirSync(dir)) {
    if (SKIP_DIRS.has(e)) continue;
    const p = join(dir, e);
    const st = statSync(p);
    if (st.isDirectory()) walk(p, out);
    else if (TEXT_EXT.has(extname(p)) || e === "pnpm-lock.yaml") out.push(p);
  }
  return out;
}

// The tests themselves quote the forbidden strings; skip them, and skip the
// Rust test modules that do the same.
const files = walk(APPS).filter((f) => !f.includes(`${require("node:path").sep}tests${require("node:path").sep}`));
const source = (f: string) => {
  const t = readFileSync(f, "utf8");
  const i = t.indexOf("#[cfg(test)]");
  return i >= 0 ? t.slice(0, i) : t;
};

describe("no hardcoded server", () => {
  // The seed node address is assembled here so this file does not contain it either.
  const SEED_PREFIX = ["186", "241"].join(".");
  it("the seed node address appears nowhere under apps/", () => {
    const hits = files.filter((f) => source(f).includes(SEED_PREFIX)).map((f) => relative(APPS, f));
    expect(hits).toEqual([]);
  });
  it("no IPv4 literal other than loopback appears in the app", () => {
    const hits: string[] = [];
    for (const f of files) {
      for (const m of source(f).matchAll(/\b(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})\b/g)) {
        const ip = m[0];
        if (ip.startsWith("127.") || ip === "0.0.0.0") continue;
        // Version-like numbers (1.2.3.4 in lockfiles) are excluded by the
        // lockfile itself being a dependency manifest, not app code.
        if (f.endsWith("pnpm-lock.yaml") || f.endsWith("Cargo.lock")) continue;
        hits.push(`${relative(APPS, f)}: ${ip}`);
      }
    }
    expect(hits).toEqual([]);
  });
});

describe("no mining", () => {
  it("the only use of the word is the negation the spec requires", () => {
    const hits: string[] = [];
    for (const f of files) {
      const text = source(f);
      for (const m of text.matchAll(/\b(mining|miner|miners|mined)\b/gi)) {
        const around = text.slice(Math.max(0, m.index! - 40), m.index! + 40).replace(/\s+/g, " ");
        if (/\bno mining\b|not mining|no\s+\w+\s+mining|is no mining|Is there mining\?\*\* No/i.test(around)) continue;
        hits.push(`${relative(APPS, f)}: …${around}…`);
      }
    }
    expect(hits, hits.join("\n")).toEqual([]);
  });
  it("the section is called Earn", () => {
    const shell = readFileSync(join(APPS, "desktop", "src", "components", "Shell.tsx"), "utf8");
    expect(shell).toMatch(/label:\s*"Earn"/);
  });
});

describe("accounts are 24 words only", () => {
  it("the UI never offers a 12-word option", () => {
    const hits = files
      .filter((f) => f.includes(`${join("desktop", "src")}`))
      .filter((f) => /\b12[- ]word/i.test(readFileSync(f, "utf8")))
      .map((f) => relative(APPS, f));
    expect(hits).toEqual([]);
  });
  it("the Rust side refuses non-24-word phrases", () => {
    const account = readFileSync(join(APPS, "..", "sdk", "rust", "hashgram-sdk", "src", "account.rs"), "utf8");
    expect(account).toContain("MNEMONIC_WORDS: usize = 24");
    const commands = readFileSync(join(APPS, "desktop", "src-tauri", "src", "commands.rs"), "utf8");
    expect(commands).toContain("words.len() != MNEMONIC_WORDS");
  });
  it("the mnemonic is never copied to the clipboard", () => {
    const onboarding = readFileSync(join(APPS, "desktop", "src", "routes", "Onboarding.tsx"), "utf8");
    expect(onboarding).not.toMatch(/copyText\([^)]*words/);
    expect(onboarding).not.toMatch(/copySensitive/);
  });
});

describe("genesis pin", () => {
  it("the mainnet genesis hash is compiled in and referenced by the network manager", () => {
    const net = readFileSync(join(APPS, "desktop", "src-tauri", "src", "net.rs"), "utf8");
    expect(net).toContain("MAINNET_GENESIS_HASH");
    expect(net).toContain("e322bc2319f6e0173286fa526dab5a8ff8ad0797c7b80dd03e7c9d98621d5e4d");
    expect(net).toContain("wrong network");
  });
});

describe("no telemetry", () => {
  it("the frontend never fetches anything itself", () => {
    const hits = files
      .filter((f) => f.includes(`${join("desktop", "src")}${require("node:path").sep}`) && (f.endsWith(".ts") || f.endsWith(".tsx")))
      .filter((f) => /\bfetch\(|XMLHttpRequest|navigator\.sendBeacon|new WebSocket\(/.test(readFileSync(f, "utf8")))
      .map((f) => relative(APPS, f));
    expect(hits).toEqual([]);
  });
});
