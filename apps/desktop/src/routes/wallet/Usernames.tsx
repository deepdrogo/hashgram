// Usernames on chain (x/username): availability, register, renew,
// transfer, release; expiry and grace shown.
import { createResource, createSignal, Show } from "solid-js";
import { useSearchParams } from "@solidjs/router";
import { Card, Button, Field, Input, Notice, Skeleton } from "~/components/ui";
import { Mono } from "~/components/identity";
import { TxConfirm } from "~/components/TxConfirm";
import { useChain, valueOf } from "~/lib/chain";
import { ipc, pick, str, arr, num, coin, type MsgSpec, type WalletOverview } from "~/lib/ipc";
import { formatHash, isHashAddress, blocksToDuration } from "~/lib/format";

export function Usernames(props: { overview: WalletOverview | null | undefined; onChanged: () => void }) {
  const [params] = useSearchParams();
  const [name, setName] = createSignal(String(params.register ?? ""));
  const [spec, setSpec] = createSignal<MsgSpec | null>(null);
  const [transferTo, setTransferTo] = createSignal("");
  const [chainParams] = useChain(() => "hashgram/username/v1/params");
  const [mine, { refetch: refetchMine }] = useChain(() => (props.overview?.username ? `hashgram/username/v1/lookup/${props.overview.username}` : null));
  const [avail] = createResource(
    () => name().trim().replace(/^@/, "").toLowerCase(),
    async (n) => {
      if (n.length < 3) return null;
      try {
        return await ipc.chainGet(`hashgram/username/v1/availability/${n}`);
      } catch (e) {
        return { error: String(e) };
      }
    },
  );
  const p = () => {
    const c = chainParams();
    const v = c?.ok ? pick(c.value, "params") : null;
    return {
      fee: str(pick(v, "registration_fee.amount")) || str(pick(v, "fee.amount")) || "1000000",
      validity: num(pick(v, "validity_blocks"), 7_884_000),
      grace: num(pick(v, "grace_blocks"), 648_000),
      min: num(pick(v, "min_length"), 3),
      max: num(pick(v, "max_length"), 32),
    };
  };
  const availability = () => {
    const a = avail();
    if (!a || "error" in a) return null;
    return {
      available: pick(a.value, "available") === true,
      reason: str(pick(a.value, "reason")),
      confusable: [...arr(pick(a.value, "confusable_with")).map((x) => str(x)), str(pick(a.value, "conflicting_name"))].filter(Boolean),
    };
  };
  const [height] = useChain(() => "cosmos/base/tendermint/v1beta1/blocks/latest");
  const currentHeight = () => num(pick(valueOf(height()), "block.header.height"));
  const registration = () => {
    const m = mine();
    if (!m?.ok) return null;
    const v = m.value;
    return {
      name: str(pick(v, "name")) || str(pick(v, "registration.name")) || props.overview?.username || "",
      expiry: num(pick(v, "expiry_height")) || num(pick(v, "registration.expiry_height")),
      transferable: pick(v, "transferable") ?? pick(v, "registration.transferable"),
    };
  };

  return (
    <div class="grid grid-cols-2 gap-4">
      <Card title="Register a username">
        <div class="flex flex-col gap-3 p-4">
          <Field label="Name" hint={`${p().min}–${p().max} characters: lowercase letters, digits, underscore. Fee ${formatHash(p().fee)} HASH, valid ${p().validity.toLocaleString()} blocks (≈ ${blocksToDuration(p().validity)}), grace ${p().grace.toLocaleString()} blocks (≈ ${blocksToDuration(p().grace)}).`}>
            <div class="flex items-center gap-2">
              <span class="mono text-muted">@</span>
              <Input mono value={name()} onInput={(e) => setName(e.currentTarget.value)} placeholder="name" />
            </div>
          </Field>
          <Show when={avail.loading}>
            <Skeleton />
          </Show>
          <Show when={availability()}>
            {(a) => (
              <>
                <Notice strong={!a().available} title={a().available ? "Available" : "Not available"}>
                  {a().reason || (a().available ? "Nobody has this name." : "Taken or reserved by the chain.")}
                  <Show when={a().confusable.length}>
                    <span class="block pt-1">Looks like an existing name: {a().confusable.map((c) => `@${c}`).join(", ")}. Others may confuse the two.</span>
                  </Show>
                </Notice>
              </>
            )}
          </Show>
          <div class="flex justify-end">
            <Button disabled={!availability()?.available} onClick={() => setSpec({ type: "register_username", name: name() })}>
              Register @{name().replace(/^@/, "").toLowerCase()}
            </Button>
          </div>
        </div>
      </Card>
      <Card title="Your username">
        <div class="flex flex-col gap-3 p-4">
          <Show when={props.overview?.username} fallback={<p class="text-sm text-muted">This address owns no username. People can still find you by address.</p>}>
            <p class="mono text-lg">@{props.overview!.username}</p>
            <Show when={registration()}>
              {(r) => (
                <p class="text-xs text-muted">
                  expires at height {r().expiry.toLocaleString()}
                  <Show when={currentHeight() && r().expiry}>
                    {" "}
                    (≈ {blocksToDuration(Math.max(0, r().expiry - currentHeight()))} left, then {blocksToDuration(p().grace)} grace)
                  </Show>
                </p>
              )}
            </Show>
            <div class="flex gap-2">
              <Button variant="secondary" onClick={() => setSpec({ type: "renew_username", name: props.overview!.username! })}>
                Renew
              </Button>
              <Button variant="ghost" onClick={() => setSpec({ type: "release_username", name: props.overview!.username! })}>
                Release
              </Button>
            </div>
            <Field label="Transfer to address">
              <div class="flex gap-2">
                <Input mono value={transferTo()} onInput={(e) => setTransferTo(e.currentTarget.value)} placeholder="hash1…" />
                <Button variant="secondary" disabled={!isHashAddress(transferTo())} onClick={() => setSpec({ type: "transfer_username", name: props.overview!.username!, to: transferTo() })}>
                  Transfer
                </Button>
              </div>
            </Field>
          </Show>
          <Show when={props.overview}>
            <p class="text-xs text-muted">
              Address: <Mono text={props.overview!.address} copy />
            </p>
          </Show>
        </div>
      </Card>
      <TxConfirm spec={spec()} onClose={() => setSpec(null)} onSubmitted={() => { void refetchMine(); props.onChanged(); }} />
    </div>
  );
}
