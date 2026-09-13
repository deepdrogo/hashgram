// In-app help: Markdown bundled at build time, rendered by the Rust side.
import { createResource, For, Show } from "solid-js";
import { useNavigate, useParams } from "@solidjs/router";
import { Card, Skeleton } from "~/components/ui";
import { ipc } from "~/lib/ipc";
import { t } from "~/lib/i18n";

export function Help() {
  const params = useParams<{ slug?: string }>();
  const navigate = useNavigate();
  const slug = () => params.slug || "quick-start";
  const [pages] = createResource(() => ipc.helpList());
  const [html] = createResource(slug, (s) => ipc.helpPage(s));
  return (
    <div class="h-full overflow-auto">
      <div class="page grid grid-cols-[200px_1fr] gap-4">
        <nav aria-label="Help pages">
          <h1 class="page-title mb-3">{t("nav_help")}</h1>
          <ul class="space-y-px">
            <For each={pages() ?? []}>
              {(p) => (
                <li>
                  <button type="button" class={`row w-full rounded-md px-2.5 py-1.5 text-left text-[13px] ${slug() === p.slug ? "bg-surface-2 text-fg" : "text-muted hover:text-fg"}`} onClick={() => navigate(`/help/${p.slug}`)}>
                    {p.title}
                  </button>
                </li>
              )}
            </For>
          </ul>
        </nav>
        <Card class="min-h-[60vh]">
          <Show when={html()} fallback={<div class="p-6"><Skeleton lines={8} /></div>}>
            <article class="help-content selectable p-6" innerHTML={html()!} />
          </Show>
        </Card>
      </div>
    </div>
  );
}
