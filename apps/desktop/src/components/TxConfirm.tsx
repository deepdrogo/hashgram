// The one confirm dialog every transaction goes through: summary, the
// warnings the protocol requires (21-day unbonding, slashing, untaxed
// transfers and the 1 % of the fee), gas and fee, and which source will
// broadcast. Nothing is signed until "Confirm".
import { createResource, createSignal, Show, For } from "solid-js";
import { Button, Dialog, Notice, Skeleton } from "./ui";
import { ipc, type MsgSpec, type TxSubmitted } from "~/lib/ipc";
import { formatHash, sourceLabel } from "~/lib/format";
import { store } from "~/lib/store";

export function TxConfirm(props: {
  spec: MsgSpec | null;
  memo?: string;
  onClose: () => void;
  onSubmitted?: (r: TxSubmitted) => void;
}) {
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [preview] = createResource(
    () => props.spec,
    async (spec) => {
      setError(null);
      try {
        return await ipc.txPreview(spec);
      } catch (e) {
        setError(String(e));
        return null;
      }
    },
  );

  const submit = async () => {
    const spec = props.spec;
    if (!spec) return;
    setBusy(true);
    setError(null);
    try {
      const r = await ipc.txSubmit(spec, props.memo ?? "");
      store.toast(`Broadcast: ${r.summary}`);
      void store.refreshPending();
      props.onSubmitted?.(r);
      props.onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      open={props.spec !== null}
      onClose={props.onClose}
      title="Confirm transaction"
      description="Review before signing. Nothing has been sent yet."
      footer={
        <>
          <Button variant="secondary" onClick={props.onClose} disabled={busy()}>
            Cancel
          </Button>
          <Button onClick={submit} loading={busy()} disabled={!preview() || !!error()}>
            Confirm and sign
          </Button>
        </>
      }
    >
      <Show when={!preview.loading} fallback={<Skeleton lines={4} />}>
        <Show when={preview()}>
          {(p) => (
            <div class="flex flex-col gap-3">
              <p class="text-sm font-medium">{p().summary}</p>
              <For each={p().warnings}>{(w) => <Notice strong={w.includes("21 days")}>{w}</Notice>}</For>
              <dl class="grid grid-cols-[auto_1fr] gap-x-6 gap-y-1 text-xs">
                <dt class="text-muted">Network fee</dt>
                <dd class="mono">
                  {formatHash(p().fee_uhash, 2, 6)} HASH{" "}
                  <span class="text-muted">({p().simulated ? "simulated" : "estimated"} gas {p().gas_limit.toLocaleString()})</span>
                </dd>
                <dt class="text-muted">Of which to the Founder (1 % of the fee)</dt>
                <dd class="mono">{formatHash(p().founder_share_uhash, 2, 6)} HASH</dd>
                <dt class="text-muted">Broadcast via</dt>
                <dd>{sourceLabel(p().source)}</dd>
                <Show when={props.memo}>
                  <dt class="text-muted">Memo</dt>
                  <dd class="selectable">{props.memo}</dd>
                </Show>
              </dl>
            </div>
          )}
        </Show>
      </Show>
      <Show when={error()}>
        <Notice strong title="Could not proceed">
          {error()}
        </Notice>
      </Show>
    </Dialog>
  );
}
