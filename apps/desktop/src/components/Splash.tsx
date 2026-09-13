// First-run splash: the mark animates into the wordmark in about a second.
// Skippable with any key or click; reduced motion shows the final frame.
import { onMount, onCleanup } from "solid-js";
import { Logo } from "./Shell";
import { t } from "~/lib/i18n";

export function Splash(props: { onDone: () => void }) {
  let done = false;
  const finish = () => {
    if (done) return;
    done = true;
    props.onDone();
  };
  onMount(() => {
    const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches || document.documentElement.dataset.reducedMotion === "true";
    const timer = setTimeout(finish, reduced ? 250 : 1150);
    const skip = () => finish();
    window.addEventListener("keydown", skip);
    window.addEventListener("pointerdown", skip);
    onCleanup(() => {
      clearTimeout(timer);
      window.removeEventListener("keydown", skip);
      window.removeEventListener("pointerdown", skip);
    });
  });
  return (
    <div class="fixed inset-0 z-50 flex items-center justify-center bg-bg" role="presentation" data-splash>
      <div class="flex flex-col items-center gap-3">
        <div class="flex items-center gap-4">
          <span class="splash-mark">
            <Logo size={52} />
          </span>
          <span class="splash-word text-3xl font-semibold tracking-tight">{t("app_name")}</span>
        </div>
        <span class="splash-word text-xs text-muted">{t("tagline")}</span>
      </div>
      <style>{`
        .splash-mark { display:inline-flex; animation: splash-mark 700ms var(--ease-out-quick) both; }
        .splash-word { animation: splash-word 600ms var(--ease-out-quick) 350ms both; }
        @keyframes splash-mark { from { transform: scale(.6) rotate(-90deg); opacity: 0 } to { transform: none; opacity: 1 } }
        @keyframes splash-word { from { transform: translateX(-8px); opacity: 0 } to { transform: none; opacity: 1 } }
      `}</style>
    </div>
  );
}
