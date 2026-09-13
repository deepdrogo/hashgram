// format.ts must agree with hashgram_sdk::wallet::{format_hash, parse_amount}
// (the vectors below are the SDK's own unit tests) and never use floats.
import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { formatHashSdk, parseAmount, formatHash, truncateMiddle, handle, splitRecipients, shortWhen } from "~/lib/format";
import { linkify, sandboxDocument } from "~/lib/linkify";
import { errText, errCode } from "~/lib/ipc";

describe("parity with hashgram_sdk::wallet", () => {
  it("parse_amount vectors", () => {
    expect(parseAmount("1")).toBe(1_000_000n);
    expect(parseAmount("1.5 HASH")).toBe(1_500_000n);
    expect(parseAmount("0.000001")).toBe(1n);
    expect(parseAmount("2500000uhash")).toBe(2_500_000n);
    expect(parseAmount(".5")).toBe(500_000n);
    expect(parseAmount("1.1234567")).toBeNull();
    expect(parseAmount("abc")).toBeNull();
    expect(parseAmount("1000000000")).toBe(1_000_000_000_000_000n);
  });
  it("format_hash vectors", () => {
    expect(formatHashSdk(1_500_000n)).toBe("1.500000 HASH");
    expect(formatHashSdk(1n)).toBe("0.000001 HASH");
    expect(formatHashSdk("1000000000000000")).toBe("1000000000.000000 HASH");
  });
  it("the Rust side has the same vectors", () => {
    const rs = readFileSync(join(__dirname, "..", "..", "..", "sdk", "rust", "hashgram-sdk", "src", "wallet.rs"), "utf8");
    expect(rs).toContain('parse_amount("1.5 HASH").unwrap(), 1_500_000');
    expect(rs).toContain('format_hash(1_500_000), "1.500000 HASH"');
  });
  it("UI formatting trims and separates without floats", () => {
    expect(formatHash("1000000000000000")).toBe("1,000,000,000.00");
    expect(formatHash("12500000")).toBe("12.50");
    expect(formatHash("1")).toBe("0.000001");
    expect(formatHash(null)).toBe("0.00");
  });
});

describe("helpers", () => {
  it("truncates and handles", () => {
    expect(truncateMiddle("hash13t8v5nnghrvgcuuqcrt9k5wyhtqwq7fl3ynjpy", 10, 6)).toBe("hash13t8v5…3ynjpy");
    expect(handle("hash1abcdefghijklmnop", "alice")).toBe("@alice");
    expect(handle("hash1abcdefghijklmnop", "", "Alice B")).toBe("Alice B");
    expect(handle("hash1abcdefghijklmnop")).toBe("hash1abcd…mnop");
  });
  it("splits recipients as typed", () => {
    expect(splitRecipients("@a, b@hashgram.io; hash1x\n@d")).toEqual(["@a", "b@hashgram.io", "hash1x", "@d"]);
  });
  it("shortWhen never throws", () => {
    expect(typeof shortWhen(Date.now())).toBe("string");
    expect(shortWhen(0)).toBe("");
  });
});

describe("linkify and sandbox", () => {
  it("turns http(s) URLs into link parts and nothing else", () => {
    const parts = linkify("see https://example.org/a?b=1. and <b>not html</b>");
    expect(parts).toEqual([
      { kind: "text", value: "see " },
      { kind: "link", value: "https://example.org/a?b=1" },
      { kind: "text", value: ". and <b>not html</b>" },
    ]);
  });
  it("the sandbox document strips scripts, handlers and frames and forbids remote loads", () => {
    const html = `<p onclick="alert(1)">hi</p><script>alert(2)</script><img src="http://evil.example/x.png"><iframe src="http://evil.example"></iframe><form action="http://x"><input></form>`;
    const doc = sandboxDocument(html, true);
    expect(doc).not.toContain("<script");
    expect(doc).not.toContain("onclick");
    expect(doc).not.toContain("<iframe");
    expect(doc).not.toContain("<form");
    expect(doc).toContain("Content-Security-Policy");
    expect(doc).toMatch(/img-src data: cid:;/);
    expect(doc).toMatch(/script-src 'none'/);
    expect(doc).toMatch(/connect-src 'none'/);
    // The remote image tag survives as markup but the CSP forbids the load.
    expect(doc).toContain('img src="http://evil.example/x.png"');
  });
});

describe("errors shown to people", () => {
  it("reads a UiError even when a resource wrapped it in an Error", () => {
    const ui = { code: "unsupported", message: "this node does not keep social events", retryable: false };
    const wrapped = new Error("Unknown error", { cause: ui });
    expect(errText(wrapped)).toBe("this node does not keep social events");
    expect(errCode(wrapped)).toBe("unsupported");
    expect(errText(ui)).toBe(ui.message);
    expect(errText(new Error("plain"))).toBe("plain");
    expect(errText("str")).toBe("str");
    expect(errCode(new Error("plain"))).toBe("internal");
  });
});