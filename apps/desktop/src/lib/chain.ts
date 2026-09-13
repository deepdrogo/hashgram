// Raw chain reads for screens that render module answers (governance,
// founder transparency, validators). Reads go through the SDK's relay
// client; nothing here is an authority beyond what the chain says.
import { createResource } from "solid-js";
import { ipc } from "./ipc";
import { store } from "./store";

export function pick(v: unknown, path: string): unknown {
  let cur: unknown = v;
  for (const k of path.split(".")) {
    if (cur === null || cur === undefined || typeof cur !== "object") return undefined;
    cur = (cur as Record<string, unknown>)[k];
  }
  return cur;
}
export function str(v: unknown, dflt = ""): string {
  if (v === null || v === undefined) return dflt;
  if (typeof v === "string") return v;
  if (typeof v === "number" || typeof v === "boolean") return String(v);
  return dflt;
}
export function num(v: unknown, dflt = 0): number {
  const n = Number(str(v));
  return Number.isFinite(n) ? n : dflt;
}
export function arr(v: unknown): unknown[] {
  return Array.isArray(v) ? v : [];
}
/** Amount of the uhash coin in a Coin or Coin[] value. */
export function coin(v: unknown): string {
  if (Array.isArray(v)) {
    const c = v.find((x) => str(pick(x, "denom")) === "uhash");
    return c ? str(pick(c, "amount"), "0") : "0";
  }
  if (v && typeof v === "object") return str(pick(v, "amount"), "0");
  return "0";
}

export function useChain(path: () => string | null) {
  return createResource(
    () => (store.locked() ? null : { p: path(), tick: store.ticks().wallet }),
    async (k) => (k.p ? ipc.chainQuery(k.p) : null),
  );
}
