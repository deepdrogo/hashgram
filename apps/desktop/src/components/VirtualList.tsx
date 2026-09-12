// Virtualised list: only the rows on screen exist in the DOM, so a
// 10,000-item feed scrolls at 60 fps. Every long list in the app uses this.
import { createVirtualizer } from "@tanstack/solid-virtual";
import { For, type JSX } from "solid-js";

export function VirtualList<T>(props: {
  items: T[];
  estimateSize?: number;
  overscan?: number;
  class?: string;
  children: (item: T, index: number) => JSX.Element;
  key?: (item: T, index: number) => string | number;
}) {
  let parent!: HTMLDivElement;
  const virtualizer = createVirtualizer({
    get count() {
      return props.items.length;
    },
    getScrollElement: () => parent,
    estimateSize: () => props.estimateSize ?? 56,
    overscan: props.overscan ?? 8,
    getItemKey: (i) => (props.key ? props.key(props.items[i] as T, i) : i),
  });
  return (
    <div ref={parent} class={`h-full overflow-auto ${props.class ?? ""}`} style={{ contain: "strict" }}>
      <div style={{ height: `${virtualizer.getTotalSize()}px`, position: "relative", width: "100%" }}>
        <For each={virtualizer.getVirtualItems()}>
          {(row) => (
            <div
              data-index={row.index}
              ref={(el) => queueMicrotask(() => virtualizer.measureElement(el))}
              style={{ position: "absolute", top: 0, left: 0, width: "100%", transform: `translateY(${row.start}px)` }}
            >
              {props.children(props.items[row.index] as T, row.index)}
            </div>
          )}
        </For>
      </div>
    </div>
  );
}
