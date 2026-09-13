// Rules from docs/DESKTOP_APP_MASTER_PROMPT.md §0 that a grep can enforce.
import { describe, it, expect } from "vitest";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, extname, relative, sep } from "node:path";

const APPS = join(__dirname, "..", "..");
const DESKTOP = join(APPS, "desktop");
const SKIP_DIRS = new Set(["node_modules", "dist", "target", "gen", ".pnpm", "icons", "binaries", "assets"]);
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

const files = walk(APPS).filter((f) => !f.includes(`${sep}tests${sep}`));
const source = (f: string) => {
  const t = readFileSync(f, "utf8");
  const i = t.indexOf("#[cfg(test)]");
  return i >= 0 ? t.slice(0, i) : t;
};
const srcFiles = files.filter((f) => f.startsWith(join(DESKTOP, "src") + sep) && (f.endsWith(".ts") || f.endsWith(".tsx")));

describe("no hardcoded server", () => {
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
        if (f.endsWith("pnpm-lock.yaml") || f.endsWith("Cargo.lock")) continue;
        hits.push(`${relative(APPS, f)}: ${ip}`);
      }
    }
    expect(hits).toEqual([]);
  });
  it("chain reads default to the P2P relay (no REST host compiled in)", () => {
    const session = readFileSync(join(DESKTOP, "src-tauri", "src", "session.rs"), "utf8");
    expect(session).toMatch(/chain_api:\s*if chain_api\.is_empty\(\)\s*\{\s*None/);
    const settings = readFileSync(join(DESKTOP, "src-tauri", "src", "settings.rs"), "utf8");
    expect(settings).toMatch(/chain_api:\s*String::new\(\)/);
  });
});

describe("no mining", () => {
  it("the only use of the word is the negation the spec requires", () => {
    const hits: string[] = [];
    for (const f of files) {
      const text = source(f);
      for (const m of text.matchAll(/\b(mining|miner|miners|mined)\b/gi)) {
        const around = text.slice(Math.max(0, m.index! - 40), m.index! + 40).replace(/\s+/g, " ");
        if (/\bno mining\b|not mining|no\s+\w+\s+mining|is no mining|there is no mining|never the word "mining"|"mining"|Is there mining\?\*\* No/i.test(around)) continue;
        hits.push(`${relative(APPS, f)}: …${around}…`);
      }
    }
    expect(hits, hits.join("\n")).toEqual([]);
  });
  it("the section is called Earn and the rail is in the specified order", () => {
    const shell = readFileSync(join(DESKTOP, "src", "components", "Shell.tsx"), "utf8");
    const keys = [...shell.matchAll(/key:\s*"nav_([a-z]+)"/g)].map((m) => m[1]);
    // Explore and My profile were added later; the original sections keep their relative order.
    expect(keys).toEqual(["mail", "drive", "feed", "explore", "people", "spaces", "earn", "wallet", "network", "me", "settings"]);
    expect(keys.filter((k) => !["explore", "me"].includes(k ?? ""))).toEqual(["mail", "drive", "feed", "people", "spaces", "earn", "wallet", "network", "settings"]);
  });
});

describe("accounts are 24 words only", () => {
  it("the UI never offers a 12-word option", () => {
    const hits = srcFiles.filter((f) => /\b12[- ]word/i.test(readFileSync(f, "utf8"))).map((f) => relative(APPS, f));
    expect(hits).toEqual([]);
  });
  it("the Rust side refuses non-24-word phrases", () => {
    const account = readFileSync(join(APPS, "..", "sdk", "rust", "hashgram-sdk", "src", "account.rs"), "utf8");
    expect(account).toContain("MNEMONIC_WORDS: usize = 24");
    const commands = readFileSync(join(DESKTOP, "src-tauri", "src", "cmd_identity.rs"), "utf8");
    expect(commands).toContain("words.len() != MNEMONIC_WORDS");
  });
  it("the mnemonic is never copied to the clipboard and blocks copy events", () => {
    const onboarding = readFileSync(join(DESKTOP, "src", "routes", "Onboarding.tsx"), "utf8");
    expect(onboarding).not.toMatch(/copyText\([^)]*words/);
    expect(onboarding).not.toMatch(/copySensitive/);
    const display = readFileSync(join(DESKTOP, "src", "components", "MnemonicDisplay.tsx"), "utf8");
    expect(display).toContain("onCopy={block}");
    expect(display).toContain("onCut={block}");
    expect(display).toMatch(/addEventListener\("blur"/);
  });
  it("there is no forgot-password reset anywhere", () => {
    const hits = srcFiles.filter((f) => /reset (your|the) password|password reset link|forgot password\?/i.test(readFileSync(f, "utf8"))).map((f) => relative(APPS, f));
    expect(hits).toEqual([]);
  });
});

describe("genesis pin", () => {
  it("the mainnet genesis hash is compiled in through the SDK and used by the session", () => {
    const session = readFileSync(join(DESKTOP, "src-tauri", "src", "session.rs"), "utf8");
    expect(session).toContain("MAINNET_GENESIS_HASH");
    const mainnet = readFileSync(join(APPS, "..", "node", "hashgram-net", "src", "mainnet.rs"), "utf8");
    expect(mainnet).toContain("e322bc2319f6e0173286fa526dab5a8ff8ad0797c7b80dd03e7c9d98621d5e4d");
  });
});

describe("no telemetry, no remote loads", () => {
  it("the frontend never fetches anything itself", () => {
    const hits = srcFiles.filter((f) => /\bfetch\(|XMLHttpRequest|navigator\.sendBeacon|new WebSocket\(/.test(readFileSync(f, "utf8"))).map((f) => relative(APPS, f));
    expect(hits).toEqual([]);
  });
  it("index.html loads nothing remote", () => {
    const html = readFileSync(join(DESKTOP, "index.html"), "utf8");
    expect(html).not.toMatch(/https?:\/\//);
  });
  it("the CSP allows only IPC connections and the capability file no shell/fs/http", () => {
    const conf = JSON.parse(readFileSync(join(DESKTOP, "src-tauri", "tauri.conf.json"), "utf8"));
    const csp: string = conf.app.security.csp;
    expect(csp).toContain("connect-src ipc: http://ipc.localhost");
    expect(csp).toContain("script-src 'self'");
    expect(csp).toContain("frame-ancestors 'none'");
    const cap = JSON.parse(readFileSync(join(DESKTOP, "src-tauri", "capabilities", "default.json"), "utf8"));
    const perms: string[] = cap.permissions;
    for (const p of perms) {
      expect(p, p).not.toMatch(/^(shell|fs|http):/);
    }
    for (const required of ["dialog:allow-open", "dialog:allow-save", "notification:default", "opener:allow-open-url", "updater:default", "deep-link:default", "clipboard-manager:allow-write-text"]) {
      expect(perms).toContain(required);
    }
  });
});

describe("HTML mail renders only in a sandbox", () => {
  it("the iframe has an empty sandbox and never receives a URL", () => {
    const f = readFileSync(join(DESKTOP, "src", "components", "HtmlSandbox.tsx"), "utf8");
    expect(f).toContain('sandbox=""');
    expect(f).toContain("srcdoc");
    expect(f).not.toMatch(/\bsrc=\{/);
    expect(f).not.toMatch(/sandbox="[^"]*allow-/);
  });
  it("body_html is only ever passed to HtmlSandbox", () => {
    const hits: string[] = [];
    for (const f of srcFiles) {
      const t = readFileSync(f, "utf8");
      if (/innerHTML=\{[^}]*body_html/.test(t)) hits.push(relative(APPS, f));
    }
    expect(hits).toEqual([]);
  });
});
