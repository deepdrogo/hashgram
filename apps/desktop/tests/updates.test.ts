// The self-updater must be pinned to signed manifests from the project's own
// GitHub Releases; a placeholder key or a foreign endpoint is a release bug.
import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { compareVersions } from "~/lib/updates";

const conf = JSON.parse(readFileSync(resolve(__dirname, "../src-tauri/tauri.conf.json"), "utf8"));

describe("updater configuration", () => {
  it("compiles in a real minisign public key", () => {
    const pk: string = conf.plugins.updater.pubkey;
    expect(pk.startsWith("REPLACE_WITH")).toBe(false);
    const text = Buffer.from(pk, "base64").toString("utf8");
    expect(text).toMatch(/^untrusted comment: minisign public key: [0-9A-F]{16}\n/);
  });

  it("fetches the manifest only from the project's GitHub Releases over HTTPS", () => {
    const eps: string[] = conf.plugins.updater.endpoints;
    expect(eps).toEqual(["https://github.com/deepdrogo/hashgram_windows/releases/latest/download/latest.json"]);
  });

  it("produces signed updater artefacts and installs passively for the current user", () => {
    expect(conf.bundle.createUpdaterArtifacts).toBe(true);
    expect(conf.plugins.updater.windows.installMode).toBe("passive");
    expect(conf.bundle.windows.nsis.installMode).toBe("currentUser");
  });
});

describe("compareVersions", () => {
  it("orders dotted versions numerically", () => {
    expect(compareVersions("0.1.1", "0.1.0")).toBeGreaterThan(0);
    expect(compareVersions("v0.2.0", "0.1.9")).toBeGreaterThan(0);
    expect(compareVersions("0.10.0", "0.9.0")).toBeGreaterThan(0);
    expect(compareVersions("1.0.0", "1.0.0")).toBe(0);
    expect(compareVersions("0.1.0", "0.1.1")).toBeLessThan(0);
  });
});
