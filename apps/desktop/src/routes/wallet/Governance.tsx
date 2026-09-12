// Governance: proposals from /cosmos/gov/v1/proposals, tally, vote.
import { createMemo, createSignal, For, Show } from "solid-js";
import { Card, Button, Notice, Skeleton, Empty, Badge } from "~/components/ui";
import { VerifiedBy } from "~/components/identity";
import { TxConfirm } from "~/components/TxConfirm";
import { useChain, readOf, errorOf } from "~/lib/chain";
import { pick, str, arr, type MsgSpec } from "~/lib/ipc";
import { formatIso, pct, toBig } from "~/lib/format";

const OPTIONS = ["yes", "no", "abstain", "no_with_veto"] as const;

export function Governance() {
  const [proposals, { refetch }] = useChain(() => "cosmos/gov/v1/proposals?pagination.limit=50&pagination.reverse=true");
  const [tallyParams] = useChain(() => "cosmos/gov/v1/params/tallying");
  const [spec, setSpec] = createSignal<MsgSpec | null>(null);

  const list = createMemo(() => {
    const p = proposals();
    if (!p?.ok) return [];
    return arr(pick(p.value, "proposals")).map((x) => {
      const tally = pick(x, "final_tally_result");
      const yes = toBig(str(pick(tally, "yes_count"), "0"));
      const no = toBig(str(pick(tally, "no_count"), "0"));
      const abstain = toBig(str(pick(tally, "abstain_count"), "0"));
      const veto = toBig(str(pick(tally, "no_with_veto_count"), "0"));
      const total = yes + no + abstain + veto;
      return {
        id: str(pick(x, "id")),
        title: str(pick(x, "title")) || str(pick(x, "metadata")) || `Proposal ${str(pick(x, "id"))}`,
        summary: str(pick(x, "summary")),
        status: str(pick(x, "status")).replace("PROPOSAL_STATUS_", "").toLowerCase(),
        votingEnd: str(pick(x, "voting_end_time")),
        yes,
        no,
        abstain,
        veto,
        total,
      };
    });
  });
  const params = () => {
    const t = tallyParams();
    const p = t?.ok ? pick(t.value, "params") ?? pick(t.value, "tally_params") : null;
    return {
      quorum: str(pick(p, "quorum"), "0.40"),
      threshold: str(pick(p, "threshold"), "0.50"),
      veto: str(pick(p, "veto_threshold"), "0.334"),
    };
  };
  const pctOf = (s: string) => `${(Number(s) * 100).toFixed(1)} %`;

  return (
    <div class="flex flex-col gap-4">
      <div class="grid grid-cols-4 gap-3">
        <div class="stat"><span class="stat-label">Voting period</span><span class="stat-value">7 days</span></div>
        <div class="stat"><span class="stat-label">Quorum</span><span class="stat-value">{pctOf(params().quorum)}</span></div>
        <div class="stat"><span class="stat-label">Threshold</span><span class="stat-value">{pctOf(params().threshold)}</span></div>
        <div class="stat"><span class="stat-label">Veto</span><span class="stat-value">{pctOf(params().veto)}</span></div>
      </div>
      <Card title="Proposals" actions={<VerifiedBy verification={readOf(proposals())?.verification} source={readOf(proposals())?.source} />}>
        <Show when={!proposals.loading} fallback={<div class="p-4"><Skeleton lines={4} /></div>}>
          <Show when={list().length} fallback={<Empty title="No proposals">{errorOf(proposals()) ?? "Nothing has been proposed on this chain yet."}</Empty>}>
            <ul>
              <For each={list()}>
                {(p) => (
                  <li class="border-b border-border p-4 last:border-0">
                    <div class="flex items-start justify-between gap-4">
                      <div class="min-w-0">
                        <p class="flex items-center gap-2 text-sm font-medium">
                          <span class="mono text-muted">#{p.id}</span>
                          <span class="truncate">{p.title}</span>
                          <Badge strong={p.status === "voting_period"}>{p.status.replace(/_/g, " ")}</Badge>
                        </p>
                        <Show when={p.summary}>
                          <p class="mt-1 line-clamp-3 text-xs text-muted selectable">{p.summary}</p>
                        </Show>
                        <p class="mt-1 text-xs text-muted">voting ends {formatIso(p.votingEnd)}</p>
                      </div>
                      <Show when={p.status === "voting_period"}>
                        <div class="flex shrink-0 gap-1">
                          <For each={OPTIONS}>
                            {(o) => (
                              <Button size="sm" variant="secondary" onClick={() => setSpec({ type: "vote", proposal_id: Number(p.id), option: o })}>
                                {o.replace(/_/g, " ")}
                              </Button>
                            )}
                          </For>
                        </div>
                      </Show>
                    </div>
                    <Show when={p.total > 0n}>
                      <div class="mt-3 flex h-1.5 overflow-hidden rounded-full bg-surface-2" role="img" aria-label="tally">
                        <div class="bg-fg" style={{ width: pct(p.yes, p.total) }} title={`yes ${pct(p.yes, p.total)}`} />
                        <div class="bg-muted" style={{ width: pct(p.no, p.total) }} title={`no ${pct(p.no, p.total)}`} />
                        <div class="bg-accent" style={{ width: pct(p.veto, p.total) }} title={`veto ${pct(p.veto, p.total)}`} />
                      </div>
                      <p class="mono mt-1 text-xs text-muted">
                        yes {pct(p.yes, p.total)} · no {pct(p.no, p.total)} · abstain {pct(p.abstain, p.total)} · veto {pct(p.veto, p.total)}
                      </p>
                    </Show>
                  </li>
                )}
              </For>
            </ul>
          </Show>
        </Show>
      </Card>
      <Notice>Votes are weighted by delegated stake. A vote can be changed until the voting period ends; the last one counts.</Notice>
      <TxConfirm spec={spec()} onClose={() => setSpec(null)} onSubmitted={() => void refetch()} />
    </div>
  );
}
