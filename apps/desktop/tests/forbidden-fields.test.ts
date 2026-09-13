// The frontend never receives keys. The TypeScript types in src/lib/ipc.ts
// mirror the Rust views; none may carry a field whose name suggests key
// material. The one exception is onboarding_generate, which returns the
// words once and whose result type is string[] (no field name at all).
import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { join } from "node:path";

const FORBIDDEN = /\b(seed|secret|mnemonic|private_key|privkey|nonce|base_nonce|manifest_key|object_key|passphrase_hash|mls_state)\b\s*[:?]/g;
// `key` alone is too common a word in prose; forbid it as a field name.
const KEY_FIELD = /^\s*(key|keys)\??:\s/m;

describe("forbidden fields in command return types", () => {
  const ipc = readFileSync(join(__dirname, "..", "src", "lib", "ipc.ts"), "utf8");
  const typesOnly = ipc.slice(0, ipc.indexOf("export const ipc = {"));

  it("no interface in ipc.ts declares a key-material field", () => {
    const hits = [...typesOnly.matchAll(FORBIDDEN)].map((m) => m[0]);
    expect(hits).toEqual([]);
    expect(KEY_FIELD.test(typesOnly)).toBe(false);
  });

  it("the Rust views strip keys before serialising", () => {
    const views = readFileSync(join(__dirname, "..", "src-tauri", "src", "views.rs"), "utf8");
    // Every capability / attachment / media view is built from the pb type
    // and never derives from it: the fields are copied by hand.
    expect(views).toContain("pub struct CapabilityView");
    expect(views).toContain("pub struct AttachmentView");
    expect(views).toContain("pub struct MediaView");
    expect(views).toContain("pub struct VersionView");
    // No view struct embeds a raw pb::DriveCapability, pb::BlobRef or pb::DriveObjectRef.
    const structs = views.split(/pub struct /).slice(1);
    for (const s of structs) {
      const body = s.slice(0, s.indexOf("\n}"));
      expect(body, s.split("\n")[0]).not.toMatch(/:\s*(app::)?(DriveCapability|BlobRef|DriveObjectRef|DriveKey|MailAttachment)\b/);
      expect(body, s.split("\n")[0]).not.toMatch(/Vec<(app::)?(DriveCapability|BlobRef|DriveObjectRef|MailAttachment)>/);
    }
  });

  it("commands that return SDK records directly return only key-free types", () => {
    // Types the Rust commands pass through unchanged. Each is public data
    // by construction (addresses, names, counts, hex ids/hashes).
    const passthrough = ["MailSummary", "FolderCounts", "EntryView", "DriveUsage", "ContactRecord", "Resolved", "Profile", "FeedItem", "PostThread", "CircleInfo", "SpaceSummary", "SpaceMember", "ProviderStatus", "Earnings", "Balance", "DeviceInfo", "PeerView"];
    for (const name of passthrough) {
      const i = typesOnly.indexOf(`export interface ${name} `);
      expect(i, name).toBeGreaterThan(-1);
      const body = typesOnly.slice(i, typesOnly.indexOf("\n}", i));
      expect(body, name).not.toMatch(FORBIDDEN);
    }
  });

  it("the composer holds attachments by index only", () => {
    const composer = readFileSync(join(__dirname, "..", "src", "routes", "mail", "Composer.tsx"), "utf8");
    expect(composer).not.toMatch(/inline_data|InlineData|\.blob\b|\.nonce\b|object_key|manifest_key|plaintext:/);
    expect(composer).toContain("mailAttachFile(");
    expect(composer).toContain("mailAttachDrive(");
  });
});
