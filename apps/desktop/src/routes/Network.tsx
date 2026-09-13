// Network: peers with roles and operator, rejected peers greyed with the
// reason, chain height and verification, supply and reserve, indexer
// leaderboards labelled as such, diagnostics export.
import { For, Show, createResource, createSignal } from "solid-js";
import { RefreshCw, Download, Network as NetIcon, ShieldCheck, ShieldAlert } from "lucide-solid";
import { Button, Card, Notice, Stat, Badge, Skeleton, Tabs } from "~/components/ui";
import { ErrorState } from "~/components/States";
import { Mono } from "~/components/identity";
import { ipc, errText, type NetworkOverview } from "~/lib/ipc";
import { store } from "~/lib/store";
import { t } from "~/lib/i18n";
import { formatHash } from "~/lib/format";
import { pick, str, arr, coin } from "~/lib/chain";
import { pickSavePath } from "~/lib/dialogs";

export function NetworkRoute() {
  const [ov, { refetch }] = createResource(
    () => store.ticks().network,
    () => ipc.networkOverview(),
  );
  const [supply] = createResource(
    () => (store.locked() ? null : store.ticks().network),
    () => ipc.networkSupply().catch(() => null),
  );
  const [board, setBoard] = createSignal<"holders" | "validators" | "providers" | "earners">("holders");
  const [top] = createResource(
    () => ({ b: board(), on: ov()?.indexer_configured ?? false, locked: store.locked() }),
    (k) => (k.on && !k.locked ? ipc.networkTop(k.b, 25).catch(() => null) : Promise.resolve(null)),
  );
  const [stats] = createResource(
    () => ({ on: ov()?.indexer_configured ?? false, locked: store.locked() }),
    (k) => (k.on && !k.locked ? ipc.networkStats().catch(() => null) : Promise.resolve(null)),
  );
  const [busy, setBusy] = createSignal(false);
  const exportDiag = async () => {
    setBusy(true);
    try {
      const text = await ipc.diagnosticsExport();
      const p = await pickSavePath("hashgram-diagnostics.txt");
      if (p) {
        await ipc.saveTextFile(p, text);
        store.toast("Diagnostics saved (no IP addresses, no contact addresses)");
      }
    } catch (e) {
      store.toast(errText(e), "error");
    } finally {
      setBusy(false);
    }
  };
  const rows = () => arr(pick(top(), "rows") ?? pick(top(), "items") ?? top());
  return (
    <div class="h-full overflow-auto">
      <div class="page flex flex-col gap-4">
        <div class="flex items-center justify-between">
          <h1 class="page-title">{t("nav_network")}</h1>
          <div class="flex gap-2">
            <Button variant="secondary" size="sm" onClick={() => void ipc.netReconnect().then(() => store.toast("Reconnecting…"))}>
              <RefreshCw size={12} /> Reconnect
            </Button>
            <Button variant="ghost" size="sm" onClick={() => void ipc.netForgetPeers().then(() => store.toast("Peers forgotten; dialling the built-in list"))}>
              Forget peers
            </Button>
            <Button variant="ghost" size="sm" onClick={exportDiag} loading={busy()}>
              <Download size={12} /> Export diagnostics
            </Button>
          </div>
        </div>
        <Show when={!ov.error} fallback={<ErrorState error={ov.error} onRetry={() => void refetch()} />}>
          <Show when={ov()} fallback={<Skeleton lines={4} />}>
            {(n: () => NetworkOverview) => (
              <>
                <div class="grid grid-cols-4 gap-3">
                  <Stat label={t("network_peers")} value={String(n().peers.length)} sub={`${n().store_peers} store · ${n().relay_peers} relay · ${n().operators} operator${n().operators === 1 ? "" : "s"}`} />
                  <Stat label="Chain height" value={n().height?.toLocaleString() ?? "—"} sub={n().verification ?? (store.locked() ? "unlock to read the chain" : "no read yet")} />
                  <Stat label="Chain" mono={false} value={n().chain_id} sub={n().network_id} />
                  <Stat label="Genesis" mono={false} value={<Mono text={n().genesis_hash} head={10} tail={8} copy />} sub="pinned at build time" />
                </div>
                <Show when={!n().link_up}>
                  <Notice strong title="The network link is not up">{n().link_error ?? "starting…"} The app keeps retrying with a growing pause.</Notice>
                </Show>
                <Show when={n().link_up && n().operators === 1}>
                  <Notice>Only one operator is reachable. Chain reads are cross-checked between nodes when two operators answer; with one, the note says so. That is a warning, not a badge.</Notice>
                </Show>
                <div class="grid grid-cols-2 gap-4">
                  <Card title={t("network_peers")}>
                    <ul>
                      <For each={n().peers} fallback={<li class="p-4 text-xs text-muted">No verified peer yet. Nodes appear as they complete the handshake.</li>}>
                        {(p) => (
                          <li class="flex items-center gap-2 border-b border-border px-3 py-2 text-xs last:border-0">
                            <ShieldCheck size={12} class="text-brand" />
                            <Mono text={p.peer} head={10} tail={6} class="flex-1" />
                            <span class="text-muted">{p.roles.join(", ")}</span>
                            <Show when={p.operator}>
                              <Mono text={p.operator} head={8} tail={4} class="text-muted" />
                            </Show>
                          </li>
                        )}
                      </For>
                    </ul>
                  </Card>
                  <Card title={t("network_rejected")}>
                    <ul>
                      <For each={n().rejected} fallback={<li class="p-4 text-xs text-muted">No peer was rejected.</li>}>
                        {([peer, reason]) => (
                          <li class="flex items-center gap-2 border-b border-border px-3 py-2 text-xs text-muted opacity-70 last:border-0">
                            <ShieldAlert size={12} />
                            <Mono text={peer} head={10} tail={6} class="flex-1" />
                            <span>{reason}</span>
                          </li>
                        )}
                      </For>
                    </ul>
                  </Card>
                </div>
                <Show when={supply()}>
                  {(s) => (
                    <div class="grid grid-cols-3 gap-3">
                      <Stat label="Total supply" value={`${formatHash(coin(pick(s(), "supply.amount")), 0, 0)} HASH`} sub="fixed" />
                      <Stat label="Service reserve" value={`${formatHash(coin(pick(s(), "service_reserve.reserve") ?? pick(s(), "service_reserve")) || "0", 0, 0)} HASH`} sub="pays providers for proven work" />
                      <Stat label="Founder revenue (paid)" value={`${formatHash(coin(pick(s(), "founder_revenue.total_paid") ?? pick(s(), "founder_revenue.paid")) || "0")} HASH`} sub="1 % of protocol fees" />
                    </div>
                  )}
                </Show>
                <Card
                  title="Leaderboards"
                  actions={<span class="text-[11px] text-muted">{n().indexer_configured ? "from the indexer you configured" : "needs an indexer URL (Settings → Network)"}</span>}
                >
                  <Show when={n().indexer_configured} fallback={<p class="p-4 text-xs text-muted">Top holders, validators and providers come from a public indexer — a read model, not an authority. None is configured.</p>}>
                    <Tabs class="px-3" value={board()} onChange={(v) => setBoard(v as "holders" | "validators" | "providers" | "earners")} tabs={[{ id: "holders", label: "Holders" }, { id: "validators", label: "Validators" }, { id: "providers", label: "Providers" }, { id: "earners", label: "Earners" }]} />
                    <table class="table">
                      <thead>
                        <tr>
                          <th>#</th>
                          <th>Address</th>
                          <th class="text-right">Amount</th>
                          <th>Kind</th>
                        </tr>
                      </thead>
                      <tbody>
                        <For each={rows()} fallback={<tr><td colSpan={4} class="text-center text-xs text-muted">{top.loading ? t("loading") : "The indexer returned nothing."}</td></tr>}>
                          {(r) => (
                            <tr>
                              <td class="tnum">{str(pick(r, "rank"))}</td>
                              <td class="flex items-center gap-2">
                                <Mono text={str(pick(r, "address") ?? pick(r, "operator") ?? pick(r, "validator"))} head={12} tail={6} copy />
                                <Show when={pick(r, "verified") === true && str(pick(r, "username"))}>
                                  <Badge brand title="registered on chain by this address">@{str(pick(r, "username"))}</Badge>
                                </Show>
                              </td>
                              <td class="tnum text-right">{formatHash(str(pick(r, "amount") ?? pick(r, "tokens") ?? pick(r, "total_paid"), "0"))}</td>
                              <td class="text-xs text-muted">{str(pick(r, "kind"))}</td>
                            </tr>
                          )}
                        </For>
                      </tbody>
                    </table>
                    <Show when={stats()}>
                      <pre class="mono border-t border-border p-3 text-[11px] text-muted selectable">{JSON.stringify(stats(), null, 1).slice(0, 2000)}</pre>
                    </Show>
                  </Show>
                </Card>
                <p class="flex items-center gap-1 text-[11px] text-muted">
                  <NetIcon size={11} /> Bootstrap peers are compiled into the app from the Mainnet parameter set; the peerstore remembers nodes it met. Nothing is pinned to one server.
                </p>
              </>
            )}
          </Show>
        </Show>
      </div>
    </div>
  );
}
