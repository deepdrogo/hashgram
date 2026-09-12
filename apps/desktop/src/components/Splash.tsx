// First-run splash: the mark animates into the wordmark in about a second.
// Skippable with any key or click; reduced motion shows the final frame.
import { onMount, onCleanup } from "solid-js";

export function Splash(props: { onDone: () => void }) {
  let done = false;
  const finish = () => {
    if (done) return;
    done = true;
    props.onDone();
  };
  onMount(() => {
    const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches || document.documentElement.dataset.reducedMotion === "true";
    const t = setTimeout(finish, reduced ? 250 : 1150);
    const skip = () => finish();
    window.addEventListener("keydown", skip);
    window.addEventListener("pointerdown", skip);
    onCleanup(() => {
      clearTimeout(t);
      window.removeEventListener("keydown", skip);
      window.removeEventListener("pointerdown", skip);
    });
  });
  return (
    <div class="fixed inset-0 z-50 flex items-center justify-center bg-bg" role="presentation" data-splash>
      <div class="flex items-center gap-4">
        <svg viewBox="0 0 64 64" width="56" height="56" aria-hidden="true" class="splash-mark">
          <rect x="21" y="12" width="6" height="40" fill="#ffffff" />
          <rect x="37" y="12" width="6" height="40" fill="#ffffff" />
          <rect x="12" y="21" width="40" height="6" fill="#ffffff" />
          <rect x="12" y="37" width="40" height="6" fill="#ffffff" />
        </svg>
        <span class="splash-word text-3xl font-semibold tracking-tight">Hashgram</span>
      </div>
      <style>{`
        .splash-mark { animation: splash-mark 700ms var(--ease-out-quick) both; }
        .splash-word { animation: splash-word 600ms var(--ease-out-quick) 350ms both; }
        @keyframes splash-mark { from { transform: scale(.6) rotate(-90deg); opacity: 0 } to { transform: none; opacity: 1 } }
        @keyframes splash-word { from { transform: translateX(-8px); opacity: 0 } to { transform: none; opacity: 1 } }
      `}</style>
    </div>
  );
}
