// UI primitives. Monochrome plus one accent; motion is short and honours
// reduced-motion. Everything here is keyboard reachable.
import { splitProps, Show, For, type JSX, type ParentProps, createEffect, onCleanup, createSignal, onMount } from "solid-js";
import { Portal } from "solid-js/web";
import { X, ChevronDown } from "lucide-solid";

type ButtonProps = JSX.ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: "primary" | "brand" | "secondary" | "ghost" | "danger";
  size?: "sm" | "md" | "icon" | "icon-sm";
  loading?: boolean;
};

export function Button(props: ParentProps<ButtonProps>) {
  const [local, rest] = splitProps(props, ["variant", "size", "loading", "class", "children", "disabled"]);
  const cls = () =>
    [
      local.variant === "secondary"
        ? "btn-secondary"
        : local.variant === "ghost"
          ? "btn-ghost"
          : local.variant === "brand"
            ? "btn-brand"
            : local.variant === "danger"
              ? "btn-danger"
              : "btn-primary",
      local.size === "sm" ? "btn-sm" : local.size === "icon" ? "btn-icon" : local.size === "icon-sm" ? "btn-icon-sm" : "",
      local.class ?? "",
    ].join(" ");
  return (
    <button type="button" class={cls()} disabled={local.disabled || local.loading} aria-busy={local.loading} {...rest}>
      <Show when={local.loading}>
        <span class="inline-block h-3 w-3 animate-spin rounded-full border border-current border-t-transparent" aria-hidden="true" />
      </Show>
      {local.children}
    </button>
  );
}

export function Card(props: ParentProps<{ class?: string; title?: string; actions?: JSX.Element }>) {
  return (
    <section class={`card ${props.class ?? ""}`}>
      <Show when={props.title}>
        <header class="flex items-center justify-between border-b border-border px-3.5 py-2">
          <h2 class="text-[13px] font-semibold">{props.title}</h2>
          {props.actions}
        </header>
      </Show>
      {props.children}
    </section>
  );
}

export function Field(props: ParentProps<{ label: string; hint?: string; error?: string; for?: string; class?: string }>) {
  return (
    <div class={props.class}>
      <label class="label" for={props.for}>
        {props.label}
      </label>
      {props.children}
      <Show when={props.error}>
        <p class="mt-1 text-xs text-fg" role="alert">
          {props.error}
        </p>
      </Show>
      <Show when={!props.error && props.hint}>
        <p class="mt-1 text-xs text-muted">{props.hint}</p>
      </Show>
    </div>
  );
}

export function Input(props: JSX.InputHTMLAttributes<HTMLInputElement> & { mono?: boolean }) {
  const [local, rest] = splitProps(props, ["class", "mono"]);
  return <input class={`input ${local.mono ? "mono" : ""} ${local.class ?? ""}`} spellcheck={false} {...rest} />;
}

export function Textarea(props: JSX.TextareaHTMLAttributes<HTMLTextAreaElement> & { mono?: boolean }) {
  const [local, rest] = splitProps(props, ["class", "mono"]);
  return <textarea class={`textarea ${local.mono ? "mono" : ""} ${local.class ?? ""}`} spellcheck={false} {...rest} />;
}

export function Select(props: { value: string; onChange: (v: string) => void; options: { value: string; label: string }[]; class?: string; disabled?: boolean; "aria-label"?: string }) {
  return (
    <span class={`relative inline-flex ${props.class ?? ""}`}>
      <select
        class="input appearance-none pr-7"
        value={props.value}
        disabled={props.disabled}
        aria-label={props["aria-label"]}
        onChange={(e) => props.onChange(e.currentTarget.value)}
      >
        <For each={props.options}>{(o) => <option value={o.value}>{o.label}</option>}</For>
      </select>
      <ChevronDown size={14} class="pointer-events-none absolute right-2 top-1/2 -translate-y-1/2 text-muted" />
    </span>
  );
}

export function Badge(props: ParentProps<{ strong?: boolean; brand?: boolean; class?: string; title?: string }>) {
  return (
    <span class={`${props.brand ? "badge-brand" : props.strong ? "badge-strong" : "badge"} ${props.class ?? ""}`} title={props.title}>
      {props.children}
    </span>
  );
}

export function Skeleton(props: { class?: string; lines?: number }) {
  return (
    <Show when={(props.lines ?? 1) > 1} fallback={<div class={`skeleton h-4 ${props.class ?? "w-full"}`} aria-hidden="true" />}>
      <div class="flex flex-col gap-2" aria-hidden="true">
        <For each={Array.from({ length: props.lines ?? 1 })}>{(_, i) => <div class={`skeleton h-4 ${i() % 3 === 2 ? "w-2/3" : "w-full"}`} />}</For>
      </div>
    </Show>
  );
}

export function Empty(props: ParentProps<{ title: string; icon?: JSX.Element; action?: JSX.Element }>) {
  return (
    <div class="flex flex-col items-center justify-center gap-2 px-6 py-12 text-center">
      <Show when={props.icon}>
        <div class="text-muted">{props.icon}</div>
      </Show>
      <p class="text-sm font-medium">{props.title}</p>
      <Show when={props.children}>
        <p class="max-w-sm text-xs text-muted">{props.children}</p>
      </Show>
      <Show when={props.action}>
        <div class="mt-2">{props.action}</div>
      </Show>
    </div>
  );
}

export function Dialog(
  props: ParentProps<{
    open: boolean;
    onClose: () => void;
    title: string;
    description?: string;
    width?: string;
    footer?: JSX.Element;
  }>,
) {
  createEffect(() => {
    if (!props.open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        props.onClose();
      }
    };
    window.addEventListener("keydown", onKey, true);
    onCleanup(() => window.removeEventListener("keydown", onKey, true));
  });
  return (
    <Show when={props.open}>
      <Portal>
        <div class="fixed inset-0 z-50 flex items-center justify-center bg-bg/80 p-6 fade-in" onClick={props.onClose} role="presentation">
          <div
            class={`card w-full ${props.width ?? "max-w-lg"} max-h-[90vh] overflow-auto shadow-none`}
            role="dialog"
            aria-modal="true"
            aria-label={props.title}
            onClick={(e) => e.stopPropagation()}
          >
            <header class="flex items-start justify-between gap-4 border-b border-border px-4 py-3">
              <div>
                <h2 class="text-sm font-semibold">{props.title}</h2>
                <Show when={props.description}>
                  <p class="mt-0.5 text-xs text-muted">{props.description}</p>
                </Show>
              </div>
              <Button variant="ghost" size="icon-sm" aria-label="Close" onClick={props.onClose}>
                <X size={14} />
              </Button>
            </header>
            <div class="px-4 py-3">{props.children}</div>
            <Show when={props.footer}>
              <footer class="flex justify-end gap-2 border-t border-border px-4 py-2.5">{props.footer}</footer>
            </Show>
          </div>
        </div>
      </Portal>
    </Show>
  );
}

export function Tabs(props: { tabs: { id: string; label: string; badge?: number; disabled?: boolean; title?: string }[]; value: string; onChange: (id: string) => void; class?: string }) {
  return (
    <div role="tablist" class={`flex gap-0.5 border-b border-border ${props.class ?? ""}`}>
      <For each={props.tabs}>
        {(t) => (
          <button
            type="button"
            role="tab"
            aria-selected={props.value === t.id}
            disabled={t.disabled}
            title={t.title}
            class={`-mb-px flex items-center gap-1.5 border-b-2 px-3 py-1.5 text-[13px] transition-colors disabled:opacity-40 ${
              props.value === t.id ? "border-brand text-fg" : "border-transparent text-muted hover:text-fg"
            }`}
            onClick={() => props.onChange(t.id)}
          >
            {t.label}
            <Show when={(t.badge ?? 0) > 0}>
              <span class="badge-strong">{t.badge}</span>
            </Show>
          </button>
        )}
      </For>
    </div>
  );
}

export function Stat(props: { label: string; value: JSX.Element; sub?: JSX.Element; mono?: boolean; title?: string }) {
  return (
    <div class="stat" title={props.title}>
      <span class="stat-label">{props.label}</span>
      <span class={props.mono === false ? "text-lg font-semibold" : "stat-value"}>{props.value}</span>
      <Show when={props.sub}>
        <span class="text-xs text-muted">{props.sub}</span>
      </Show>
    </div>
  );
}

export function Switch(props: { checked: boolean; onChange: (v: boolean) => void; label: string; hint?: string; disabled?: boolean }) {
  return (
    <label class="flex items-start justify-between gap-4 py-1.5">
      <span>
        <span class="block text-[13px]">{props.label}</span>
        <Show when={props.hint}>
          <span class="block text-xs text-muted">{props.hint}</span>
        </Show>
      </span>
      <button
        type="button"
        role="switch"
        aria-checked={props.checked}
        aria-label={props.label}
        disabled={props.disabled}
        class={`relative mt-0.5 h-5 w-9 shrink-0 rounded-full border transition-colors ${props.checked ? "border-brand bg-brand" : "border-accent bg-surface-2"} disabled:opacity-40`}
        onClick={() => props.onChange(!props.checked)}
      >
        <span class={`absolute top-0.5 h-3.5 w-3.5 rounded-full transition-[left] ${props.checked ? "left-[18px] bg-bg" : "left-0.5 bg-muted"}`} />
      </button>
    </label>
  );
}

export function Checkbox(props: { checked: boolean; onChange: (v: boolean) => void; label: string; hint?: string; disabled?: boolean }) {
  return (
    <label class="flex items-start gap-2 py-1 text-[13px]">
      <input type="checkbox" class="mt-0.5 accent-brand" checked={props.checked} disabled={props.disabled} onChange={(e) => props.onChange(e.currentTarget.checked)} />
      <span>
        {props.label}
        <Show when={props.hint}>
          <span class="block text-xs text-muted">{props.hint}</span>
        </Show>
      </span>
    </label>
  );
}

export function Kbd(props: ParentProps) {
  return <kbd class="kbd">{props.children}</kbd>;
}

export function Notice(props: ParentProps<{ title?: string; strong?: boolean; class?: string }>) {
  return (
    <div class={`rounded-md border px-3 py-2 text-xs ${props.strong ? "border-fg text-fg" : "border-border text-muted"} ${props.class ?? ""}`} role="note">
      <Show when={props.title}>
        <p class="mb-0.5 font-medium text-fg">{props.title}</p>
      </Show>
      {props.children}
    </div>
  );
}

export interface MenuItem {
  label: string;
  icon?: JSX.Element;
  onSelect?: () => void;
  disabled?: boolean;
  danger?: boolean;
  separator?: boolean;
  title?: string;
}

/** A context / dropdown menu anchored at a point. */
export function Menu(props: { open: boolean; x: number; y: number; items: MenuItem[]; onClose: () => void }) {
  let ref!: HTMLDivElement;
  const [pos, setPos] = createSignal({ x: props.x, y: props.y });
  createEffect(() => {
    if (!props.open) return;
    setPos({ x: props.x, y: props.y });
    const onDown = (e: MouseEvent) => {
      if (!ref?.contains(e.target as Node)) props.onClose();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") props.onClose();
    };
    window.addEventListener("pointerdown", onDown, true);
    window.addEventListener("keydown", onKey, true);
    queueMicrotask(() => {
      if (!ref) return;
      const r = ref.getBoundingClientRect();
      const x = Math.min(props.x, window.innerWidth - r.width - 8);
      const y = Math.min(props.y, window.innerHeight - r.height - 8);
      setPos({ x: Math.max(4, x), y: Math.max(4, y) });
    });
    onCleanup(() => {
      window.removeEventListener("pointerdown", onDown, true);
      window.removeEventListener("keydown", onKey, true);
    });
  });
  return (
    <Show when={props.open}>
      <Portal>
        <div ref={ref} class="menu fixed" role="menu" style={{ left: `${pos().x}px`, top: `${pos().y}px` }}>
          <For each={props.items}>
            {(it) => (
              <Show when={!it.separator} fallback={<div class="menu-sep" role="separator" />}>
                <button
                  type="button"
                  role="menuitem"
                  class={`menu-item ${it.danger ? "text-fg" : ""}`}
                  disabled={it.disabled}
                  title={it.title}
                  onClick={() => {
                    props.onClose();
                    it.onSelect?.();
                  }}
                >
                  <Show when={it.icon}>
                    <span class="text-muted">{it.icon}</span>
                  </Show>
                  {it.label}
                </button>
              </Show>
            )}
          </For>
        </div>
      </Portal>
    </Show>
  );
}

/** Hook-style helper: returns state + open(e) for a context menu. */
export function useContextMenu() {
  const [state, setState] = createSignal<{ open: boolean; x: number; y: number }>({ open: false, x: 0, y: 0 });
  const open = (e: MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setState({ open: true, x: e.clientX, y: e.clientY });
  };
  const openAt = (el: HTMLElement) => {
    const r = el.getBoundingClientRect();
    setState({ open: true, x: r.left, y: r.bottom + 4 });
  };
  const close = () => setState((s) => ({ ...s, open: false }));
  return { state, open, openAt, close };
}

export function ProgressBar(props: { value: number; max: number; class?: string; label?: string }) {
  const pct = () => (props.max > 0 ? Math.min(100, Math.round((props.value / props.max) * 100)) : 0);
  return (
    <div class={`h-1.5 w-full overflow-hidden rounded-full bg-surface-2 ${props.class ?? ""}`} role="progressbar" aria-valuenow={pct()} aria-valuemin={0} aria-valuemax={100} aria-label={props.label}>
      <div class="h-full bg-brand transition-[width]" style={{ width: `${pct()}%` }} />
    </div>
  );
}

/** Focuses the first input of a container on mount. */
export function autofocus(el: HTMLElement) {
  onMount(() => queueMicrotask(() => (el.querySelector("input,textarea") as HTMLElement | null)?.focus()));
}
