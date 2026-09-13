// Hidden Performance panel (Ctrl+Shift+P): Rust spans, UI marks, memory,
// against the budgets the spec sets.
import { createResource, createSignal, For, Show, onCleanup } from "solid-js";
import { Portal } from "solid-js/web";
import { ipc } from "~/lib/ipc";
import { formatBytes } from "~/lib/format";
import { Button } from "./ui";

export const BUDGET = {
  coldStartMs: 1500,
  unlockToMailMs: 400,
  idleRamBytes: 180 * 1024 * 1024,
  syncRoundMs: 3000,
};

export function PerfPanel(props: { open: boolean; onClose: () => void }) {
  const [tick, setTick] = createSignal(0);
  const [data] = createResource(
    () => (props.open ? tick() : null),
    async () => {
      const [spans, mem] = await Promise.all([ipc.perfSnapshot(), ipc.perfMemory()]);
      return { spans, mem };
    },
  );
  const timer = setInterval(() => props.open && setTick((t) => t + 1), 2000);
  onCleanup(() => clearInterval(timer));

  const uiInteractive = () => data()?.spans.find((s) => s.name === "ui:interactive");
  const slowest = () =>
    [...(data()?.spans ?? [])]
      .filter((s) => s.origin === "rust")
      .sort((a, b) => b.micros - a.micros)
      .slice(0, 12);
  const frames = () => (data()?.spans ?? []).filter((s) => s.name.startsWith("ui:frame"));

  return (
    <Show when={props.open}>
      <Portal>
        <div class="fixed bottom-9 right-3 z-40 w-[420px] card fade-in" role="dialog" aria-label="Performance">
          <header class="flex items-center justify-between border-b border-border px-3 py-2">
            <h2 class="text-xs font-medium">Performance</h2>
            <Button variant="ghost" size="sm" onClick={props.onClose}>
              Close
            </Button>
          </header>
          <div class="max-h-[50vh] overflow-auto p-3 text-xs">
            <dl class="mono grid grid-cols-[1fr_auto] gap-x-4 gap-y-1">
              <dt class="text-muted">cold start → interactive</dt>
              <dd class={uiInteractive() && uiInteractive()!.micros / 1000 > BUDGET.coldStartMs ? "text-fg" : ""}>
                {uiInteractive() ? `${(uiInteractive()!.micros / 1000).toFixed(0)} ms` : "—"} <span class="text-muted">/ {BUDGET.coldStartMs} ms</span>
              </dd>
              <dt class="text-muted">process working set</dt>
              <dd>
                {data() ? formatBytes(data()!.mem) : "—"} <span class="text-muted">/ {formatBytes(BUDGET.idleRamBytes)} (WebView2 renderer separate)</span>
              </dd>
              <dt class="text-muted">UI frames sampled</dt>
              <dd>
                {frames().length}
                <Show when={frames().length}>
                  {" "}
                  <span class="text-muted">
                    p95 {p95(frames().map((f) => f.micros / 1000)).toFixed(1)} ms
                  </span>
                </Show>
              </dd>
            </dl>
            <p class="mt-3 mb-1 text-[11px] uppercase tracking-wide text-muted">Slowest Rust spans</p>
            <table class="mono w-full">
              <For each={slowest()}>
                {(s) => (
                  <tr class="border-t border-border">
                    <td class="py-1 pr-2 text-muted">{s.name}</td>
                    <td class="py-1 text-right">{(s.micros / 1000).toFixed(1)} ms</td>
                  </tr>
                )}
              </For>
            </table>
          </div>
        </div>
      </Portal>
    </Show>
  );
}

function p95(v: number[]): number {
  if (!v.length) return 0;
  const s = [...v].sort((a, b) => a - b);
  return s[Math.min(s.length - 1, Math.floor(s.length * 0.95))] ?? 0;
}
