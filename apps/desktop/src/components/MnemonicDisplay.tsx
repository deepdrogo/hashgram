import { createSignal, For, onCleanup, onMount, Show } from "solid-js";

/**
 * Displays the one-time recovery phrase without clipboard interaction.
 * The words blur as soon as the application loses focus.
 */
export function MnemonicDisplay(props: { words: string[] }) {
  const [focused, setFocused] = createSignal(document.hasFocus());
  const block = (event: ClipboardEvent) => {
    event.preventDefault();
    event.stopPropagation();
  };

  onMount(() => {
    const show = () => setFocused(true);
    const hide = () => setFocused(false);
    const syncVisibility = () =>
      setFocused(document.visibilityState === "visible" && document.hasFocus());
    window.addEventListener("focus", show);
    window.addEventListener("blur", hide);
    document.addEventListener("visibilitychange", syncVisibility);
    onCleanup(() => {
      window.removeEventListener("focus", show);
      window.removeEventListener("blur", hide);
      document.removeEventListener("visibilitychange", syncVisibility);
    });
  });

  return (
    <div class="relative">
      <ol
        class={`grid grid-cols-3 gap-2 select-none transition-[filter] ${focused() ? "" : "blur-sm"}`}
        aria-label="Recovery words"
        aria-hidden={!focused()}
        onCopy={block}
        onCut={block}
      >
        <For each={props.words}>
          {(word, index) => (
            <li class="rounded-md border border-border bg-surface px-3 py-2 font-mono text-sm">
              <span class="mr-2 text-muted">{index() + 1}</span>
              {word}
            </li>
          )}
        </For>
      </ol>
      <Show when={!focused()}>
        <p class="absolute inset-0 flex items-center justify-center text-sm font-medium">
          Recovery words hidden while Hashgram is not focused
        </p>
      </Show>
    </div>
  );
}
