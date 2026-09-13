// Components that render a person or a hash. The anti-impersonation rule
// lives here: a display name is never shown without the verified @username
// or the middle-truncated address beside it, in the monospace face.
import { Show, createResource } from "solid-js";
import { Copy } from "lucide-solid";
import { convertFileSrc } from "@tauri-apps/api/core";
import { truncateMiddle } from "~/lib/format";
import { copyText } from "~/lib/clipboard";
import { ipc } from "~/lib/ipc";
import { store } from "~/lib/store";

export function Mono(props: { text: string; head?: number; tail?: number; copy?: boolean; class?: string; full?: boolean; title?: string }) {
  const shown = () => (props.full ? props.text : truncateMiddle(props.text, props.head ?? 10, props.tail ?? 6));
  return (
    <span class={`mono inline-flex items-center gap-1 ${props.class ?? ""}`} title={props.title ?? props.text}>
      <span class="selectable" data-full={props.text}>
        {shown()}
      </span>
      <Show when={props.copy}>
        <button
          type="button"
          class="text-muted hover:text-fg"
          aria-label={`Copy ${props.text}`}
          onClick={(e) => {
            e.stopPropagation();
            void copyText(props.text);
          }}
        >
          <Copy size={12} />
        </button>
      </Show>
    </span>
  );
}

export interface Person {
  address: string;
  /** Verified on chain (x/username). */
  username?: string | null;
  /** Self-declared. Not identity. */
  displayName?: string | null;
}

/**
 * Renders a person. The verified handle (`@name`) or the truncated address is
 * ALWAYS present, monospace, next to any display name.
 */
export function PersonLabel(props: { person: Person; size?: "sm" | "md" | "lg"; copy?: boolean; class?: string }) {
  const handle = () => (props.person.username ? `@${props.person.username}` : truncateMiddle(props.person.address, 9, 4));
  const sizeCls = () => (props.size === "lg" ? "text-base" : props.size === "sm" ? "text-xs" : "text-[13px]");
  return (
    <span class={`inline-flex min-w-0 items-baseline gap-1.5 ${sizeCls()} ${props.class ?? ""}`} data-person={props.person.address}>
      <Show when={props.person.displayName}>
        <span class="truncate font-medium" data-display-name>
          {props.person.displayName}
        </span>
      </Show>
      <span class="mono truncate text-muted" data-handle title={props.person.address}>
        {handle()}
      </span>
      <Show when={props.copy}>
        <button
          type="button"
          class="text-muted hover:text-fg"
          aria-label="Copy address"
          onClick={(e) => {
            e.stopPropagation();
            void copyText(props.person.address);
          }}
        >
          <Copy size={12} />
        </button>
      </Show>
    </span>
  );
}

/** Resolves an address to its @username through the Rust cache and renders it. */
export function Who(props: { address: string; class?: string; me?: boolean; size?: "sm" | "md" | "lg" }) {
  const isMe = () => props.me || store.status()?.address === props.address;
  const [name] = createResource(
    () => ({ a: props.address, locked: store.locked() }),
    (k) => (k.locked || !k.a ? Promise.resolve("") : ipc.peopleUsernameOf(k.a).catch(() => "")),
  );
  return (
    <span class={`inline-flex min-w-0 items-baseline gap-1.5 ${props.class ?? ""}`} title={props.address}>
      <Show when={isMe()}>
        <span class="text-muted">you</span>
      </Show>
      <span class={`mono truncate ${props.size === "sm" ? "text-xs" : ""}`}>{name() ? `@${name()}` : truncateMiddle(props.address, 9, 4)}</span>
    </span>
  );
}

export function Avatar(props: { address: string; size?: number; src?: string | null; name?: string }) {
  // Identicon: 5x5 symmetric grid from the address bytes, in the muted tone.
  const cells = () => {
    const s = props.address || "";
    const out: boolean[] = [];
    for (let i = 0; i < 15; i++) {
      const c = s.charCodeAt(5 + (i % Math.max(1, s.length - 5))) || 0;
      out.push(((c >> (i % 5)) & 1) === 1);
    }
    return out;
  };
  const size = () => props.size ?? 28;
  return (
    <Show
      when={props.src}
      fallback={
        <svg width={size()} height={size()} viewBox="0 0 5 5" class="shrink-0 rounded-md bg-surface-2 text-muted" aria-hidden="true" shape-rendering="crispEdges">
          {cells().map((on, i) => {
            const x = i % 3;
            const y = Math.floor(i / 3);
            if (!on) return null;
            return (
              <>
                <rect x={x} y={y} width="1" height="1" fill="currentColor" />
                <rect x={4 - x} y={y} width="1" height="1" fill="currentColor" />
              </>
            );
          })}
        </svg>
      }
    >
      <img src={props.src ?? ""} width={size()} height={size()} class="shrink-0 rounded-md object-cover" alt="" />
    </Show>
  );
}

// Avatar resolution: address → profile (cached ≤15 min on the Rust side) →
// avatar blob → local file. Memoised per session so a list of fifty posts
// by the same author costs one lookup, and never repeated for an address
// that has no avatar.
const avatarCache = new Map<string, Promise<string | null>>();

export function avatarSrc(address: string): Promise<string | null> {
  let p = avatarCache.get(address);
  if (!p) {
    p = ipc
      .peopleProfileCached(address)
      .then((prof) => (prof.avatar_cid ? ipc.peopleAvatar(prof.avatar_cid).then(convertFileSrc) : null))
      .catch(() => null);
    avatarCache.set(address, p);
  }
  return p;
}

/** Forgets a cached avatar (after the user changes their own). */
export function forgetAvatar(address: string) {
  avatarCache.delete(address);
}

/** An address's network avatar, with the identicon until (or unless) one loads. */
export function PersonAvatar(props: { address: string; size?: number }) {
  const [src] = createResource(
    () => ({ a: props.address, locked: store.locked() }),
    (k) => (k.locked || !k.a ? Promise.resolve(null) : avatarSrc(k.a)),
  );
  return <Avatar address={props.address} size={props.size} src={src() ?? null} />;
}

export function Dot(props: { state: "ok" | "warn" | "bad" | "off"; title?: string; class?: string }) {
  return <span class={`dot dot-${props.state} ${props.class ?? ""}`} title={props.title} aria-label={props.title} role="img" />;
}
