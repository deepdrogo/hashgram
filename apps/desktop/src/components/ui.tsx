// Monochrome UI primitives in the spirit of shadcn/ui. No colour beyond the
// seven tokens; motion is short and honours reduced-motion.
import { splitProps, Show, For, type JSX, type ParentProps, createEffect, onCleanup } from "solid-js";
import { Portal } from "solid-js/web";
import { X } from "lucide-solid";

type ButtonProps = JSX.ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: "primary" | "secondary" | "ghost";
  size?: "sm" | "md" | "icon";
  loading?: boolean;
};

export function Button(props: ParentProps<ButtonProps>) {
  const [local, rest] = splitProps(props, ["variant", "size", "loading", "class", "children", "disabled"]);
  const cls = () =>
    [
      local.variant === "secondary" ? "btn-secondary" : local.variant === "ghost" ? "btn-ghost" : "btn-primary",
      local.size === "sm" ? "btn-sm" : local.size === "icon" ? "btn-icon" : "",
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
        <header class="flex items-center justify-between border-b border-border px-4 py-2.5">
          <h2 class="text-sm font-medium">{props.title}</h2>
          {props.actions}
        </header>
      </Show>
      {props.children}
    </section>
  );
}

export function Field(props: ParentProps<{ label: string; hint?: string; error?: string; for?: string }>) {
  return (
    <div>
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

export function Badge(props: ParentProps<{ strong?: boolean; class?: string; title?: string }>) {
  return (
    <span class={`${props.strong ? "badge-strong" : "badge"} ${props.class ?? ""}`} title={props.title}>
      {props.children}
    </span>
  );
}

export function Skeleton(props: { class?: string; lines?: number }) {
  return (
    <Show when={(props.lines ?? 1) > 1} fallback={<div class={`skeleton h-4 ${props.class ?? "w-full"}`} aria-hidden="true" />}>
      <div class="flex flex-col gap-2" aria-hidden="true">
        <For each={Array.from({ length: props.lines ?? 1 })}>
          {(_, i) => <div class={`skeleton h-4 ${i() % 3 === 2 ? "w-2/3" : "w-full"}`} />}
        </For>
      </div>
    </Show>
  );
}

export function Empty(props: ParentProps<{ title: string; icon?: JSX.Element }>) {
  return (
    <div class="flex flex-col items-center justify-center gap-2 px-6 py-12 text-center">
      <Show when={props.icon}>
        <div class="text-muted">{props.icon}</div>
      </Show>
      <p class="text-sm font-medium">{props.title}</p>
      <Show when={props.children}>
        <p class="max-w-sm text-xs text-muted">{props.children}</p>
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
      if (e.key === "Escape") props.onClose();
    };
    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));
  });
  return (
    <Show when={props.open}>
      <Portal>
        <div class="fixed inset-0 z-50 flex items-center justify-center bg-bg/80 p-6 fade-in" onClick={props.onClose} role="presentation">
          <div
            class={`card w-full ${props.width ?? "max-w-lg"} shadow-none`}
            role="dialog"
            aria-modal="true"
            aria-label={props.title}
            onClick={(e) => e.stopPropagation()}
          >
            <header class="flex items-start justify-between gap-4 border-b border-border px-5 py-4">
              <div>
                <h2 class="text-base font-semibold">{props.title}</h2>
                <Show when={props.description}>
                  <p class="mt-0.5 text-xs text-muted">{props.description}</p>
                </Show>
              </div>
              <Button variant="ghost" size="icon" aria-label="Close" onClick={props.onClose}>
                <X size={16} />
              </Button>
            </header>
            <div class="px-5 py-4">{props.children}</div>
            <Show when={props.footer}>
              <footer class="flex justify-end gap-2 border-t border-border px-5 py-3">{props.footer}</footer>
            </Show>
          </div>
        </div>
      </Portal>
    </Show>
  );
}

export function Tabs(props: { tabs: { id: string; label: string }[]; value: string; onChange: (id: string) => void }) {
  return (
    <div role="tablist" class="flex gap-1 border-b border-border">
      <For each={props.tabs}>
        {(t) => (
          <button
            type="button"
            role="tab"
            aria-selected={props.value === t.id}
            class={`-mb-px border-b-2 px-3 py-2 text-sm transition-colors ${
              props.value === t.id ? "border-fg text-fg" : "border-transparent text-muted hover:text-fg"
            }`}
            onClick={() => props.onChange(t.id)}
          >
            {t.label}
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
      <span class={props.mono === false ? "text-xl font-semibold" : "stat-value"}>{props.value}</span>
      <Show when={props.sub}>
        <span class="text-xs text-muted">{props.sub}</span>
      </Show>
    </div>
  );
}

export function Switch(props: { checked: boolean; onChange: (v: boolean) => void; label: string; hint?: string; disabled?: boolean }) {
  return (
    <label class="flex items-start justify-between gap-4 py-2">
      <span>
        <span class="block text-sm">{props.label}</span>
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
        class={`relative mt-0.5 h-5 w-9 shrink-0 rounded-full border transition-colors ${
          props.checked ? "border-fg bg-fg" : "border-accent bg-surface-2"
        } disabled:opacity-40`}
        onClick={() => props.onChange(!props.checked)}
      >
        <span
          class={`absolute top-0.5 h-3.5 w-3.5 rounded-full transition-[left] ${
            props.checked ? "left-[18px] bg-bg" : "left-0.5 bg-muted"
          }`}
        />
      </button>
    </label>
  );
}

export function Kbd(props: ParentProps) {
  return <kbd class="kbd">{props.children}</kbd>;
}

export function Notice(props: ParentProps<{ title?: string; strong?: boolean }>) {
  return (
    <div class={`rounded-md border px-3 py-2 text-xs ${props.strong ? "border-fg text-fg" : "border-border text-muted"}`} role="note">
      <Show when={props.title}>
        <p class="mb-0.5 font-medium text-fg">{props.title}</p>
      </Show>
      {props.children}
    </div>
  );
}
