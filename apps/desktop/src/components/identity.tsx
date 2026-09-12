// Components that render a person or a hash. The anti-impersonation rule
// lives here and nowhere else: a display name is never shown without the
// verified @username or the middle-truncated address beside it, in the
// monospace face. tests/person-label.test.tsx checks it.
import { Show, type JSX } from "solid-js";
import { Copy, ShieldCheck, ShieldAlert } from "lucide-solid";
import { truncateMiddle, verificationLabel } from "~/lib/format";
import { copyText } from "~/lib/clipboard";
import type { Source, Verification } from "~/lib/ipc";

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
  /** Self-declared, from a signed social event. Not identity. */
  displayName?: string | null;
}

/**
 * Renders a person. The verified handle (`@name`) or the truncated address is
 * ALWAYS present, monospace, next to any display name.
 */
export function PersonLabel(props: { person: Person; size?: "sm" | "md" | "lg"; copy?: boolean; class?: string }) {
  const handle = () => (props.person.username ? `@${props.person.username}` : truncateMiddle(props.person.address, 10, 6));
  const sizeCls = () => (props.size === "lg" ? "text-base" : props.size === "sm" ? "text-xs" : "text-sm");
  return (
    <span class={`inline-flex min-w-0 items-baseline gap-2 ${sizeCls()} ${props.class ?? ""}`} data-person={props.person.address}>
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

export function Avatar(props: { address: string; size?: number; src?: string | null }) {
  // Identicon: 5x5 symmetric grid from the address bytes, monochrome.
  const cells = () => {
    const s = props.address || "";
    const out: boolean[] = [];
    for (let i = 0; i < 15; i++) {
      const c = s.charCodeAt(5 + (i % Math.max(1, s.length - 5)));
      out.push(((c >> (i % 5)) & 1) === 1);
    }
    return out;
  };
  const size = () => props.size ?? 32;
  return (
    <Show
      when={props.src}
      fallback={
        <svg width={size()} height={size()} viewBox="0 0 5 5" class="shrink-0 rounded-md bg-surface-2" aria-hidden="true" shape-rendering="crispEdges">
          {cells().map((on, i) => {
            const x = i % 3;
            const y = Math.floor(i / 3);
            if (!on) return null;
            return (
              <>
                <rect x={x} y={y} width="1" height="1" fill="#808080" />
                <rect x={4 - x} y={y} width="1" height="1" fill="#808080" />
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

export function HealthDot(props: { state: "ok" | "warn" | "bad" | "off"; title?: string }) {
  return <span class={`health-dot health-${props.state}`} title={props.title} aria-label={props.title} role="img" />;
}

/** "verified by 2 nodes" or the honest alternative. */
export function VerifiedBy(props: { verification: Verification | null | undefined; source?: Source | null; class?: string }): JSX.Element {
  const ok = () => !!props.verification?.agreed;
  const label = () => verificationLabel(props.verification, props.source);
  return (
    <span class={`inline-flex items-center gap-1 text-xs ${ok() ? "text-fg" : "text-muted"} ${props.class ?? ""}`} title={props.verification?.peers.join("\n")}>
      <Show when={ok()} fallback={<ShieldAlert size={12} />}>
        <ShieldCheck size={12} />
      </Show>
      {label()}
    </span>
  );
}
