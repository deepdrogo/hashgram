// Identity & devices: MsgCreateIdentity / MsgAddDevice / MsgRevokeDevice,
// recovery config with the mandatory delay and cancel.
import { createResource, createSignal, For, Show } from "solid-js";
import { Card, Button, Field, Input, Notice, Skeleton, Badge } from "~/components/ui";
import { Mono } from "~/components/identity";
import { TxConfirm } from "~/components/TxConfirm";
import { ipc, pick, str, arr, num, type MsgSpec } from "~/lib/ipc";
import { store } from "~/lib/store";
import { isHashAddress, blocksToDuration } from "~/lib/format";

export function Identity() {
  const [status, { refetch }] = createResource(async () => {
    try {
      return { ok: await ipc.identityStatus(), error: null as string | null };
    } catch (e) {
      return { ok: null, error: String(e) };
    }
  });
  const [busy, setBusy] = createSignal(false);
  const [label, setLabel] = createSignal(store.settings()?.device_label ?? "");
  const [spec, setSpec] = createSignal<MsgSpec | null>(null);
  const [guardians, setGuardians] = createSignal("");
  const [threshold, setThreshold] = createSignal("1");
  const [delay, setDelay] = createSignal("21600");

  const register = async () => {
    setBusy(true);
    try {
      const r = await ipc.identityRegister(label());
      store.toast(r.summary);
      void refetch();
    } catch (e) {
      store.toast(String(e), "error");
    } finally {
      setBusy(false);
    }
  };
  const recovery = () => status()?.ok?.recovery;
  const pending = () => {
    const r = recovery();
    const req = pick(r, "request") ?? pick(r, "pending");
    return req && typeof req === "object" ? req : null;
  };
  const guardianList = () => guardians().split(/[\s,]+/).map((g) => g.trim()).filter(Boolean);
  const guardiansOk = () => guardianList().length > 0 && guardianList().every(isHashAddress) && Number(threshold()) >= 1 && Number(threshold()) <= guardianList().length;

  return (
    <div class="flex flex-col gap-4">
      <Show when={status()} fallback={<Skeleton lines={4} />}>
        {(s) => (
          <Show when={s().ok} fallback={<Notice strong title="Identity unavailable">{s().error}</Notice>}>
            {(id) => (
              <>
                <div class="grid grid-cols-3 gap-3">
                  <div class="stat">
                    <span class="stat-label">Identity on chain</span>
                    <span class="text-xl font-semibold">{id().registered ? "registered" : "not yet"}</span>
                    <Show when={id().rotation_count !== null}>
                      <span class="text-xs text-muted">root rotations: {id().rotation_count}</span>
                    </Show>
                  </div>
                  <div class="stat">
                    <span class="stat-label">This PC</span>
                    <span class="text-xl font-semibold">{id().this_device_registered ? "registered" : "not registered"}</span>
                    <Mono text={id().this_device_pubkey} head={10} tail={6} copy class="text-xs text-muted" />
                  </div>
                  <div class="stat">
                    <span class="stat-label">Devices</span>
                    <span class="stat-value">{id().devices.filter((d) => !d.revoked).length}</span>
                    <span class="text-xs text-muted">{id().devices.filter((d) => d.revoked).length} revoked</span>
                  </div>
                </div>

                <Show when={!id().this_device_registered}>
                  <Card title={id().registered ? "Add this PC as a device" : "Register your identity"}>
                    <div class="flex flex-col gap-3 p-4">
                      <p class="text-sm text-muted">
                        {id().registered
                          ? "This account already has an identity on chain (created from another device or an earlier install). Your identity root key, derived from the 24 words, signs a certificate for this PC's device key and MsgAddDevice registers it."
                          : "MsgCreateIdentity puts your identity root public key and this PC's device public key on chain. That is what lets anyone verify your messages and posts without a server."}{" "}
                        Only public keys go on chain. The transaction needs a little HASH for the fee.
                      </p>
                      <Field label="Device label">
                        <Input value={label()} onInput={(e) => setLabel(e.currentTarget.value)} placeholder="Home PC" />
                      </Field>
                      <div class="flex justify-end">
                        <Button onClick={register} loading={busy()}>
                          {id().registered ? "Add device (MsgAddDevice)" : "Create identity (MsgCreateIdentity)"}
                        </Button>
                      </div>
                    </div>
                  </Card>
                </Show>

                <Card title="Devices">
                  <Show when={id().devices.length} fallback={<p class="p-4 text-sm text-muted">No devices on chain.</p>}>
                    <table class="table">
                      <thead>
                        <tr>
                          <th>Label</th>
                          <th>Platform</th>
                          <th>Device id</th>
                          <th>Public key</th>
                          <th />
                        </tr>
                      </thead>
                      <tbody>
                        <For each={id().devices}>
                          {(d) => (
                            <tr class={d.revoked ? "opacity-50" : ""}>
                              <td>
                                <span class="flex items-center gap-2">
                                  {d.label || "(no label)"}
                                  <Show when={d.is_this_device}>
                                    <Badge strong>this PC</Badge>
                                  </Show>
                                  <Show when={d.revoked}>
                                    <Badge>revoked</Badge>
                                  </Show>
                                </span>
                              </td>
                              <td>{d.platform}</td>
                              <td>
                                <Mono text={d.device_id} head={10} tail={4} />
                              </td>
                              <td>
                                <Mono text={d.pubkey_hex} head={10} tail={6} copy />
                              </td>
                              <td class="text-right">
                                <Show when={!d.revoked && !d.is_this_device}>
                                  <Button size="sm" variant="ghost" onClick={() => setSpec({ type: "revoke_device", device_id: d.device_id })}>
                                    Revoke
                                  </Button>
                                </Show>
                              </td>
                            </tr>
                          )}
                        </For>
                      </tbody>
                    </table>
                  </Show>
                </Card>

                <Card title="Recovery">
                  <div class="flex flex-col gap-3 p-4">
                    <Show when={pending()}>
                      <Notice strong title="A recovery is pending for this identity">
                        Executable at height {str(pick(pending(), "executable_height"))} — if you did not ask for this, cancel it now. The delay exists so you can.
                        <div class="pt-2">
                          <Button size="sm" onClick={() => setSpec({ type: "cancel_recovery" })}>
                            Cancel recovery (MsgCancelRecovery)
                          </Button>
                        </div>
                      </Notice>
                    </Show>
                    <Show when={arr(pick(recovery(), "config.guardians")).length || arr(pick(recovery(), "recovery.guardians")).length}>
                      <p class="text-xs text-muted">
                        Guardians: {arr(pick(recovery(), "config.guardians") ?? pick(recovery(), "recovery.guardians")).map((g) => str(g)).join(", ")} · threshold {str(pick(recovery(), "config.threshold") ?? pick(recovery(), "recovery.threshold"))} · delay {num(pick(recovery(), "config.recovery_delay_blocks") ?? pick(recovery(), "recovery.recovery_delay_blocks")).toLocaleString()} blocks
                      </p>
                    </Show>
                    <p class="text-sm text-muted">Social recovery lets a set of guardian addresses jointly move this identity to a new key after a mandatory delay. The delay is your window to cancel a recovery you did not ask for.</p>
                    <Field label="Guardian addresses (space or comma separated)">
                      <Input mono value={guardians()} onInput={(e) => setGuardians(e.currentTarget.value)} placeholder="hash1… hash1…" />
                    </Field>
                    <div class="grid grid-cols-2 gap-3">
                      <Field label="Threshold (approvals needed)">
                        <Input mono type="number" min="1" value={threshold()} onInput={(e) => setThreshold(e.currentTarget.value)} />
                      </Field>
                      <Field label="Delay in blocks" hint={`≈ ${blocksToDuration(Number(delay()) || 0)} at ~4 s/block`}>
                        <Input mono type="number" min="1" value={delay()} onInput={(e) => setDelay(e.currentTarget.value)} />
                      </Field>
                    </div>
                    <div class="flex justify-end">
                      <Button variant="secondary" disabled={!guardiansOk() || !id().registered} onClick={() => setSpec({ type: "set_recovery_config", guardians: guardianList(), threshold: Number(threshold()), delay_blocks: Number(delay()) })}>
                        Set recovery config
                      </Button>
                    </div>
                  </div>
                </Card>
              </>
            )}
          </Show>
        )}
      </Show>
      <TxConfirm spec={spec()} onClose={() => setSpec(null)} onSubmitted={() => void refetch()} />
    </div>
  );
}
