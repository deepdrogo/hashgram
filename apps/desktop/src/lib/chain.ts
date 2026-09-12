// Reactive chain reads for screens. Each read goes through the Rust side's
// precedence, cross-check and short cache.
import { createResource, type Accessor } from "solid-js";
import { ipc, type ChainRead } from "./ipc";

export type ChainState<T = unknown> = { ok: true; read: ChainRead; value: T } | { ok: false; error: string };

export function useChain<T = unknown>(path: Accessor<string | null | undefined>) {
  return createResource<ChainState<T> | null, string>(
    () => path() ?? undefined,
    async (p) => {
      try {
        const read = await ipc.chainGet(p);
        return { ok: true, read, value: read.value as T };
      } catch (e) {
        return { ok: false, error: String(e) };
      }
    },
  );
}

export const readOf = (s: ChainState | null | undefined): ChainRead | null => (s && s.ok ? s.read : null);
export const valueOf = (s: ChainState | null | undefined): unknown => (s && s.ok ? s.value : undefined);
export const errorOf = (s: ChainState | null | undefined): string | null => (s && !s.ok ? s.error : null);

export function useChainMany(paths: Accessor<string[] | null | undefined>) {
  return createResource<Array<ChainState> | null, string[]>(
    () => paths() ?? undefined,
    async (ps) => {
      const out = await ipc.chainGetMany(ps);
      return out.map((r) => ("Ok" in r ? { ok: true as const, read: r.Ok, value: r.Ok.value } : { ok: false as const, error: r.Err }));
    },
  );
}
