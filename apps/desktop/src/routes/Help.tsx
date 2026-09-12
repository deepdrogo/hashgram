import { createResource, createSignal, For, Show } from "solid-js";
import { Card, Skeleton } from "~/components/ui";
import { ipc } from "~/lib/ipc";

export function Help() {
  const [pages] = createResource(() => ipc.helpList());
  const [slug, setSlug] = createSignal("quick-start");
  const [html] = createResource(slug, (s) => ipc.helpPage(s));
  return (
    <div class="page grid grid-cols-[220px_1fr] gap-4">
      <nav aria-label="Help pages">
        <h1 class="page-title mb-3">Help</h1>
        <ul class="space-y-0.5">
          <For each={pages() ?? []}>
            {(p) => (
              <li>
                <button type="button" class={`w-full rounded-md px-2.5 py-1.5 text-left text-sm ${slug() === p.slug ? "bg-surface-2 text-fg" : "text-muted hover:text-fg"}`} onClick={() => setSlug(p.slug)}>
                  {p.title}
                </button>
              </li>
            )}
          </For>
        </ul>
      </nav>
      <Card class="min-h-[60vh]">
        <Show when={html()} fallback={<div class="p-6"><Skeleton lines={8} /></div>}>
          <article class="help prose-mono selectable p-6 text-sm leading-relaxed" innerHTML={html()!} />
        </Show>
      </Card>
      <style>{`
        .help h1 { font-size: 1.25rem; font-weight: 600; margin: 0 0 .75rem; letter-spacing: -.01em }
        .help h2 { font-size: 1rem; font-weight: 600; margin: 1.25rem 0 .5rem }
        .help p { margin: .5rem 0; color: var(--color-fg) }
        .help ul, .help ol { margin: .5rem 0 .5rem 1.25rem; list-style: disc }
        .help ol { list-style: decimal }
        .help li { margin: .2rem 0 }
        .help code { font-family: var(--font-mono); font-size: .85em; background: var(--color-surface-2); padding: 0 .3em; border-radius: 4px }
        .help table { width: 100%; border-collapse: collapse; margin: .75rem 0; font-size: .85rem }
        .help th, .help td { border-bottom: 1px solid var(--color-border); padding: .35rem .5rem; text-align: left }
        .help th { color: var(--color-muted); font-weight: 500 }
        .help strong { font-weight: 600 }
        .help a { text-decoration: underline; text-underline-offset: 2px }
        .help .help-source { margin-top: 1.5rem; color: var(--color-muted); font-size: .75rem }
      `}</style>
    </div>
  );
}
