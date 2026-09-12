// Who is who: verified @username (chain) and self-declared display name
// (latest signed PROFILE_UPDATE), cached per address for the session.
import { createSignal } from "solid-js";
import { ipc, pick, str } from "./ipc";
import type { Person } from "~/components/identity";

const cache = new Map<string, Person>();
const [version, setVersion] = createSignal(0);
const pending = new Set<string>();

/** Reactive lookup; kicks off resolution the first time. */
export function person(address: string): Person {
  version();
  const hit = cache.get(address);
  if (hit) return hit;
  if (!pending.has(address)) {
    pending.add(address);
    void resolve(address);
  }
  return { address };
}

async function resolve(address: string) {
  const p: Person = { address };
  try {
    const r = await ipc.chainGet(`hashgram/username/v1/reverse/${address}`);
    const name = str(pick(r.value, "name"));
    if (name) p.username = name;
  } catch {
    /* no username or no chain source */
  }
  try {
    const prof = await ipc.profileGet(address, false);
    const dn = str(pick(prof.profile, "display_name"));
    if (dn) p.displayName = dn;
  } catch {
    /* no profile */
  }
  cache.set(address, p);
  pending.delete(address);
  setVersion((v) => v + 1);
}

/** Forgets one address (after a profile update). */
export function forget(address: string) {
  cache.delete(address);
  setVersion((v) => v + 1);
}
