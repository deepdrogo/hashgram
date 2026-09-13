// Component behaviour against mocked commands: composer recipient chips,
// Requests folder actions, Drive share dialog, Space role gating, Circle
// poll tally, the HTML sandbox.
import { describe, it, expect, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor, cleanup } from "@solidjs/testing-library";
import { Router, Route } from "@solidjs/router";
import { mockCommand } from "./setup";
import { Composer } from "~/routes/mail/Composer";
import { HtmlSandbox } from "~/components/HtmlSandbox";
import { CirclePost } from "~/routes/feed/Feed";
import { can } from "~/routes/spaces/Spaces";
import { ROLE, type DraftView, type CircleItemView, type RecipientResolution } from "~/lib/ipc";

const draft = (over: Partial<DraftView> = {}): DraftView => ({
  id: "d1",
  to: [],
  cc: [],
  bcc: [],
  subject: "",
  body_text: "",
  body_html: "",
  attachments: [],
  in_reply_to: "",
  updated_at_ms: 0,
  ...over,
});

beforeEach(() => {
  cleanup();
  mockCommand("settings_get", () => null);
  mockCommand("touch", () => undefined);
  mockCommand("perf_mark", () => undefined);
  mockCommand("ui_log", () => undefined);
});

describe("composer recipient chips", () => {
  it("resolves typed recipients into chips and blocks send while one is invalid", async () => {
    let saved: DraftView = draft();
    mockCommand("mail_draft_new", () => saved);
    mockCommand("mail_draft_save", (a) => (saved = draft({ ...(a?.fields as object) })));
    mockCommand("mail_resolve_recipients", (a) => {
      const inputs = a?.inputs as string[];
      return inputs.map<RecipientResolution>((input) =>
        input === "@alice"
          ? { input, kind: "hashgram", address: "hash1alice", username: "alice", display_name: "", has_identity: true, devices: 1, error: "" }
          : input === "bob@example.com"
            ? { input, kind: "external", address: "bob@example.com", username: "", display_name: "", has_identity: false, devices: 0, error: "external e-mail needs a mail gateway; set one in Settings → Network" }
            : { input, kind: "invalid", address: "", username: "", display_name: "", has_identity: false, devices: 0, error: "no such username" },
      );
    });
    render(() => <Composer open={{}} onClose={() => undefined} onSent={() => undefined} />);
    const input = (await screen.findByPlaceholderText("@name, name@hashgram.io or hash1…")) as HTMLInputElement;
    fireEvent.input(input, { target: { value: "@alice, bob@example.com" } });
    fireEvent.blur(input);
    await waitFor(() => expect(document.querySelectorAll("[data-chip]").length).toBe(2));
    await waitFor(() => expect(document.querySelector('[data-chip="@alice"]')?.getAttribute("data-kind")).toBe("hashgram"));
    expect(document.querySelector('[data-chip="@alice"]')?.textContent).toContain("@alice");
    expect(document.querySelector('[data-chip="bob@example.com"]')?.getAttribute("data-kind")).toBe("external");
    // The external chip carries an error (no gateway) → send disabled and the problem listed.
    const send = screen.getByText("Send").closest("button")!;
    expect(send.disabled).toBe(true);
    expect(screen.getByTestId("composer-problems").textContent).toContain("gateway");
    // Remove the external chip → send enabled.
    fireEvent.click(document.querySelector('[data-chip="bob@example.com"] button')!);
    await waitFor(() => expect((screen.getByText("Send").closest("button") as HTMLButtonElement).disabled).toBe(false));
  });
});

describe("HTML sandbox", () => {
  it("renders body_html only through an iframe with sandbox=\"\" and executes nothing", async () => {
    let executed = false;
    (window as unknown as { __x: () => void }).__x = () => (executed = true);
    render(() => <HtmlSandbox html={`<p>hi</p><script>parent.__x()</script><img src="http://evil.example/track.png">`} />);
    const frame = document.querySelector("iframe")!;
    expect(frame.getAttribute("sandbox")).toBe("");
    expect(frame.getAttribute("src")).toBeNull();
    await waitFor(() => expect(frame.srcdoc).toContain("Content-Security-Policy"));
    expect(frame.srcdoc).not.toContain("<script");
    expect(executed).toBe(false);
    // Nothing in the host document points at the remote image.
    expect(document.querySelector('img[src^="http"]')).toBeNull();
  });
});

describe("circle poll tally", () => {
  it("renders votes and percentages from Item.votes", async () => {
    mockCommand("circles_comments", () => []);
    mockCommand("people_username_of", () => "");
    mockCommand("app_status", () => ({ address: "hash1me" }));
    const item: CircleItemView = {
      id: "p1",
      kind: "post",
      author: "hash1alice",
      at_ms: Date.now(),
      text: "Lunch?",
      post_id: "",
      reply_to: "",
      media: [],
      drive_refs: [],
      poll: { question: "Where?", options: ["Pizza", "Sushi", "Salad"], multiple_choice: false, closes_at_ms: 0 },
      reactions: {},
      votes: { "0": 3, "1": 1 },
      deleted: false,
    };
    render(() => <CirclePost circle="c1" item={item} />);
    await screen.findByText("Where?");
    const opts = document.querySelectorAll("[data-option]");
    expect(opts.length).toBe(3);
    expect(opts[0]!.querySelector("[data-votes]")!.textContent).toContain("3 · 75%");
    expect(opts[1]!.querySelector("[data-votes]")!.textContent).toContain("1 · 25%");
    expect(opts[2]!.querySelector("[data-votes]")!.textContent).toContain("0 · 0%");
    expect(document.querySelector("[data-poll]")!.textContent).toContain("4 votes");
  });
});

describe("space role gating", () => {
  it("mirrors hashgram_app::space rules", () => {
    // Guest: read only.
    expect(can.post(ROLE.guest)).toBe(false);
    expect(can.comment(ROLE.guest)).toBe(false);
    expect(can.shareDrive(ROLE.guest)).toBe(false);
    expect(can.invite(ROLE.guest)).toBe(false);
    // Member: post/comment/share, no admin acts.
    expect(can.post(ROLE.member)).toBe(true);
    expect(can.announce(ROLE.member)).toBe(false);
    expect(can.invite(ROLE.member)).toBe(false);
    expect(can.inviteRoles(ROLE.member)).toEqual([]);
    // Admin: invite up to Member, announce, remove lower, set ≤ Member on targets < Admin.
    expect(can.announce(ROLE.admin)).toBe(true);
    expect(can.inviteRoles(ROLE.admin)).toEqual([ROLE.guest, ROLE.member]);
    expect(can.remove(ROLE.admin, ROLE.member)).toBe(true);
    expect(can.remove(ROLE.admin, ROLE.admin)).toBe(false);
    expect(can.remove(ROLE.admin, ROLE.owner)).toBe(false);
    expect(can.setRoles(ROLE.admin, ROLE.member, false)).toEqual([ROLE.guest, ROLE.member]);
    expect(can.setRoles(ROLE.admin, ROLE.admin, false)).toEqual([]);
    expect(can.setRoles(ROLE.admin, ROLE.admin, true)).toEqual([]);
    // Owner: everything incl. transfer; cannot leave; cannot change own role.
    expect(can.inviteRoles(ROLE.owner)).toEqual([ROLE.guest, ROLE.member, ROLE.admin]);
    expect(can.setRoles(ROLE.owner, ROLE.admin, false)).toContain(ROLE.owner);
    expect(can.setRoles(ROLE.owner, ROLE.owner, true)).toEqual([]);
    expect(can.leave(ROLE.owner)).toBe(false);
    expect(can.leave(ROLE.member)).toBe(true);
    expect(can.remove(ROLE.owner, ROLE.admin)).toBe(true);
    // Unshare: the sharer or an admin.
    expect(can.unshare(ROLE.member, true)).toBe(true);
    expect(can.unshare(ROLE.member, false)).toBe(false);
    expect(can.unshare(ROLE.admin, false)).toBe(true);
  });
});

describe("requests folder actions", () => {
  it("Accept moves to inbox, Block blocks the authenticated sender then trashes", async () => {
    const calls: string[] = [];
    mockCommand("mail_get", () => ({
      id: "m1", thread_id: "t1", folder: "requests", from: { address: "hash1eve", username: "", display_name: "" }, authenticated_sender: "hash1eve", sender_matches: true,
      to: [], cc: [], bcc_copy: false, created_at_ms: 1, received_at_ms: 1, subject: "hello", body_text: "hi", body_html: "", attachments: [], in_reply_to: "", references: [],
      external: null, request_read_receipt: false, importance: 0, sender_labels: [], labels: [], read: true, starred: false, delivered_to: {}, read_by: {}, outgoing: false, trust_score: -5, expire_after_secs: 0,
    }));
    mockCommand("mail_thread", () => null);
    mockCommand("mail_live_attachment_versions", () => ({}));
    mockCommand("people_username_of", () => "");
    mockCommand("mail_counts", () => ({}));
    mockCommand("people_list", () => []);
    mockCommand("mail_accept_request", (a) => calls.push(`accept:${a?.id}`));
    mockCommand("people_block", (a) => calls.push(`block:${a?.address}`));
    mockCommand("mail_trash", (a) => calls.push(`trash:${a?.id}`));
    const { Reader } = await import("~/routes/mail/Reader");
    render(() => (
      <Router root={(p) => <>{p.children}</>}>
        <Route path="*" component={() => <Reader id="m1" folder="requests" threaded={false} onReply={() => undefined} onForward={() => undefined} onChanged={() => undefined} />} />
      </Router>
    ));
    const accept = await screen.findByText("Accept");
    fireEvent.click(accept);
    await waitFor(() => expect(calls).toContain("accept:m1"));
    (window as unknown as { confirm: () => boolean }).confirm = () => true;
    fireEvent.click(screen.getByText("Block sender"));
    await waitFor(() => expect(calls).toContain("block:hash1eve"));
    expect(calls).toContain("trash:m1");
  });
});
