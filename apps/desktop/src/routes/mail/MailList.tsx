// The message list: virtualised rows with authenticated sender, subject,
// preview, time, attachment icon, star, External badge and BCC chip.
import { Show, createMemo } from "solid-js";
import { Paperclip, Star } from "lucide-solid";
import { VirtualList } from "~/components/VirtualList";
import { Empty } from "~/components/ui";
import { Who } from "~/components/identity";
import { shortWhen } from "~/lib/format";
import { ipc, type MailSummary } from "~/lib/ipc";
import { t } from "~/lib/i18n";
import { store } from "~/lib/store";

export function MailList(props: {
  items: MailSummary[];
  selected: string | null;
  onSelect: (m: MailSummary) => void;
  onContext?: (m: MailSummary, e: MouseEvent) => void;
  emptyTitle: string;
  emptyHint?: string;
}) {
  const me = createMemo(() => store.status()?.address ?? "");
  return (
    <Show when={props.items.length} fallback={<Empty title={props.emptyTitle}>{props.emptyHint}</Empty>}>
      <VirtualList items={props.items} estimateSize={64} key={(m) => m.id} class="h-full">
        {(m) => (
          <div
            role="row"
            tabIndex={-1}
            data-id={m.id}
            data-selected={props.selected === m.id}
            class={`row flex cursor-default flex-col gap-0.5 border-b border-border px-3 py-2 ${m.read ? "" : "bg-surface"}`}
            onClick={() => props.onSelect(m)}
            onContextMenu={(e) => props.onContext?.(m, e)}
          >
            <div class="flex items-center gap-2">
              <Show when={!m.read}>
                <span class="dot dot-ok shrink-0" aria-label="unread" />
              </Show>
              <span class={`min-w-0 flex-1 truncate text-[13px] ${m.read ? "" : "font-semibold"}`}>
                {/* The authenticated sender resolved on chain — never the claimed From hint. */}
                <Show when={m.outgoing} fallback={<Who address={m.from} />}>
                  <span class="text-muted">To </span>
                  <Show when={m.to[0]} fallback={<span class="text-muted">—</span>}>
                    <Who address={m.to[0]!} me={m.to[0] === me()} />
                  </Show>
                  <Show when={m.to.length > 1}>
                    <span class="text-muted"> +{m.to.length - 1}</span>
                  </Show>
                </Show>
              </span>
              <span class="tnum shrink-0 text-[11px] text-muted">{shortWhen(m.received_at_ms)}</span>
            </div>
            <div class="flex items-center gap-1.5">
              <span class={`min-w-0 flex-1 truncate text-[13px] ${m.read ? "text-fg" : "font-medium"}`}>{m.subject || "(no subject)"}</span>
              <Show when={m.external}>
                <span class="badge" title={t("mail_external")}>
                  External
                </span>
              </Show>
              <Show when={m.bcc_copy}>
                <span class="badge">BCC</span>
              </Show>
              <Show when={m.attachments > 0}>
                <Paperclip size={12} class="text-muted" aria-label={`${m.attachments} attachments`} />
              </Show>
              <button
                type="button"
                class={`shrink-0 ${m.starred ? "text-brand" : "text-muted opacity-0 hover:opacity-100 group-hover:opacity-100"}`}
                aria-label={m.starred ? t("mail_unstar") : t("mail_star")}
                onClick={(e) => {
                  e.stopPropagation();
                  void ipc.mailStar(m.id, !m.starred).then(() => store.bump("mail"));
                }}
              >
                <Star size={13} fill={m.starred ? "currentColor" : "none"} />
              </button>
            </div>
            <div class="truncate text-xs text-muted">{m.preview.replace(/\s+/g, " ")}</div>
          </div>
        )}
      </VirtualList>
    </Show>
  );
}
