// The anti-impersonation rule: a display name is never rendered without the
// verified @username or the truncated address next to it, in monospace.
import { describe, it, expect } from "vitest";
import { render } from "@solidjs/testing-library";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, extname } from "node:path";
import { PersonLabel } from "~/components/identity";

const ADDR = "hash13t8v5nnghrvgcuuqcrt9k5wyhtqwq7fl3ynjpy";

describe("PersonLabel", () => {
  it("shows the verified @username beside a display name", () => {
    const { container } = render(() => <PersonLabel person={{ address: ADDR, username: "alice", displayName: "Satoshi Nakamoto" }} />);
    const handle = container.querySelector("[data-handle]");
    expect(handle?.textContent).toBe("@alice");
    expect(handle?.classList.contains("mono")).toBe(true);
    expect(container.querySelector("[data-display-name]")?.textContent).toBe("Satoshi Nakamoto");
  });

  it("falls back to the middle-truncated address when there is no username", () => {
    const { container } = render(() => <PersonLabel person={{ address: ADDR, displayName: "Impostor" }} />);
    const handle = container.querySelector("[data-handle]");
    expect(handle?.textContent).toBe("hash13t8v5…3ynjpy");
    expect(handle?.getAttribute("title")).toBe(ADDR);
  });

  it("never renders a display name alone", () => {
    const { container } = render(() => <PersonLabel person={{ address: ADDR, displayName: "Just A Name" }} />);
    expect(container.querySelector("[data-display-name]")).not.toBeNull();
    expect(container.querySelector("[data-handle]")).not.toBeNull();
  });

  it("a person with no display name still shows the handle", () => {
    const { container } = render(() => <PersonLabel person={{ address: ADDR, username: "bob" }} />);
    expect(container.querySelector("[data-display-name]")).toBeNull();
    expect(container.querySelector("[data-handle]")?.textContent).toBe("@bob");
  });
});

describe("every component that renders a person uses PersonLabel", () => {
  // Any file mentioning a display name must render it through PersonLabel;
  // rendering `displayName`/`display_name` directly is forbidden.
  const root = join(__dirname, "..", "src");
  const files: string[] = [];
  const walk = (d: string) => {
    for (const e of readdirSync(d)) {
      const p = join(d, e);
      if (statSync(p).isDirectory()) walk(p);
      else if (extname(p) === ".tsx" && !p.endsWith("identity.tsx")) files.push(p);
    }
  };
  walk(root);
  it("does not render display names outside PersonLabel", () => {
    const offenders: string[] = [];
    for (const f of files) {
      const text = readFileSync(f, "utf8");
      if (/\{[^}]*\b(displayName|display_name)\b[^}]*\}/.test(text) && !text.includes("PersonLabel")) offenders.push(f);
    }
    expect(offenders).toEqual([]);
  });
});
