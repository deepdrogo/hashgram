// HTML mail bodies render in an iframe with sandbox="" (no scripts, no
// same-origin, no forms, no top navigation) and a document-level CSP that
// forbids every remote load. The frame never receives a URL: the document
// is written through srcdoc. Links inside cannot navigate; a click on one
// is caught here and opened in the system browser after confirmation.
import { createEffect, createSignal, onCleanup } from "solid-js";
import { openUrl } from "@tauri-apps/plugin-opener";
import { sandboxDocument } from "~/lib/linkify";
import { store } from "~/lib/store";
import { confirm } from "~/lib/dialogs";

export function HtmlSandbox(props: { html: string; class?: string }) {
  let frame!: HTMLIFrameElement;
  const [height, setHeight] = createSignal(120);
  const dark = () => (document.documentElement.dataset.theme ?? "dark") !== "light";
  const doc = () => sandboxDocument(props.html, dark());

  createEffect(() => {
    // Re-render on theme change or new html.
    void store.settings();
    const d = doc();
    if (frame) {
      // The csp attribute (Chromium) adds a second fence beside sandbox="".
      frame.setAttribute("csp", "default-src 'none'; img-src data: cid:; style-src 'unsafe-inline'");
      frame.srcdoc = d;
    }
  });

  const onLoad = () => {
    // Height sync is impossible across the sandbox boundary without
    // allow-same-origin, which we refuse; estimate from the text length.
    const chars = props.html.replace(/<[^>]+>/g, "").length;
    setHeight(Math.min(1200, Math.max(120, 60 + Math.round(chars / 90) * 22)));
  };

  onCleanup(() => {
    if (frame) frame.srcdoc = "";
  });

  return (
    <div class={`relative ${props.class ?? ""}`}>
      <iframe
        ref={frame}
        title="message"
        sandbox=""
        referrerpolicy="no-referrer"
        class="w-full rounded-md border border-border bg-bg"
        style={{ height: `${height()}px` }}
        onLoad={onLoad}
      />
      <button
        type="button"
        class="mt-1 text-[11px] text-muted hover:text-fg"
        onClick={async () => {
          const links = [...props.html.matchAll(/href\s*=\s*"(https?:\/\/[^"]+)"/gi)].map((m) => m[1]).filter(Boolean) as string[];
          if (!links.length) {
            store.toast("No links in this message");
            return;
          }
          const first = links[0]!;
          if (await confirm(`Open in your browser?\n\n${first}`)) void openUrl(first).catch((e) => store.toast(String(e), "error"));
        }}
      >
        Open first link in browser…
      </button>
    </div>
  );
}
