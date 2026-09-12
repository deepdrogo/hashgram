// Test setup: the Tauri IPC does not exist under vitest, so `invoke` is
// mocked to reject unless a test installs handlers.
import { vi } from "vitest";

const handlers = new Map<string, (args: Record<string, unknown> | undefined) => unknown>();

export function mockCommand(name: string, fn: (args: Record<string, unknown> | undefined) => unknown) {
  handlers.set(name, fn);
}

vi.mock("@tauri-apps/api/core", () => ({
  invoke: async (cmd: string, args?: Record<string, unknown>) => {
    const h = handlers.get(cmd);
    if (!h) throw new Error(`no mock for command ${cmd}`);
    return h(args);
  },
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: async () => () => undefined,
}));

vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({
  writeText: async () => undefined,
}));

if (!("matchMedia" in window)) {
  Object.defineProperty(window, "matchMedia", {
    value: () => ({ matches: false, addEventListener: () => undefined, removeEventListener: () => undefined }),
  });
}
