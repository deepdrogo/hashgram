// Day-to-day unlock: passphrase or Windows Hello. The 24 words are never
// asked for here; "Forgot passphrase" explains restore. No reset exists.
import { createSignal, Show, onMount } from "solid-js";
import { Fingerprint, Lock as LockIcon } from "lucide-solid";
import { Button, Field, Input, Notice } from "~/components/ui";
import { Logo } from "~/components/Shell";
import { ipc, errText } from "~/lib/ipc";
import { store } from "~/lib/store";
import { t } from "~/lib/i18n";
import { truncateMiddle } from "~/lib/format";

export function Lock(props: { onUnlocked: () => void }) {
  const [pass, setPass] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [forgot, setForgot] = createSignal(false);
  const [confirm, setConfirm] = createSignal("");
  let input!: HTMLInputElement;

  const wipe = async () => {
    setBusy(true);
    setError(null);
    try {
      await ipc.wipeLocalData(confirm());
      await store.refreshStatus();
      store.toast("Local data removed. Restore from your 24 words or a backup file.");
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };

  const unlock = async () => {
    setError(null);
    setBusy(true);
    try {
      await ipc.unlock(pass());
      setPass("");
      await store.refreshStatus();
      props.onUnlocked();
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };
  const hello = async () => {
    setError(null);
    setBusy(true);
    try {
      await ipc.helloUnlock();
      await store.refreshStatus();
      props.onUnlocked();
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };
  onMount(() => {
    input?.focus();
    if (store.status()?.hello_enabled) void hello();
  });

  return (
    <div class="flex h-full items-center justify-center p-8">
      <div class="w-full max-w-sm fade-in">
        <div class="mb-6 flex items-center gap-3">
          <Logo size={26} />
          <div>
            <h1 class="text-lg font-semibold">{t("locked_title")}</h1>
            <p class="mono text-xs text-muted" title={store.status()?.address ?? ""}>
              {truncateMiddle(store.status()?.address ?? "", 14, 8)}
            </p>
          </div>
        </div>
        <div class="flex flex-col gap-3">
          <Field label={t("passphrase")} error={error() ?? undefined}>
            <Input ref={input} type="password" value={pass()} onInput={(e) => setPass(e.currentTarget.value)} onKeyDown={(e) => e.key === "Enter" && void unlock()} autocomplete="current-password" />
          </Field>
          <Button onClick={unlock} disabled={!pass()} loading={busy()}>
            <LockIcon size={14} /> {t("unlock")}
          </Button>
          <Show when={store.status()?.hello_enabled}>
            <Button variant="secondary" onClick={hello} disabled={busy()}>
              <Fingerprint size={14} /> {t("hello_unlock")}
            </Button>
          </Show>
          <Show when={!store.status()?.link_up}>
            <p class="text-xs text-muted">{store.status()?.link_error ? `Network: ${store.status()?.link_error}` : "Connecting to the network in the background…"}</p>
          </Show>
          <button type="button" class="text-xs text-muted hover:text-fg" onClick={() => setForgot((v) => !v)}>
            Lost the passphrase? There is no reset — read what you can do
          </button>
          <Show when={forgot()}>
            <Notice title="There is no reset">
              The passphrase only protects the keys on this PC. To get back in, remove this PC's local data and restore from your 24 words or a backup file. Nobody else can do this for you. Mail and files stored only on this PC are lost.
            </Notice>
            <Field label="Type DELETE to remove the local vault and start over" hint="You will need your 24 words or a backup file.">
              <Input mono value={confirm()} onInput={(e) => setConfirm(e.currentTarget.value)} />
            </Field>
            <Button variant="danger" disabled={confirm() !== "DELETE"} loading={busy()} onClick={wipe}>
              Remove local data and start over
            </Button>
          </Show>
        </div>
      </div>
    </div>
  );
}
