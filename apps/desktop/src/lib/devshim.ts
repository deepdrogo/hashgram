// Development-only: when the page runs in a plain browser (no Tauri), a
// fake IPC answers the commands with canned, obviously synthetic data so
// the screens can be looked at and clicked through. Never part of a build:
// `import.meta.env.DEV` gates it and the release bundle tree-shakes it.
// No key material exists here either — the shim mirrors the views.

type Args = Record<string, unknown> | undefined;

const now = Date.now();
const ME = "hash1me00000000000000000000000000000000000";
const ALICE = "hash1alice000000000000000000000000000000000";
const BOB = "hash1bob0000000000000000000000000000000000";
const CAROL = "hash1carol00000000000000000000000000000000";

const mails = [
  { id: "m1", thread_id: "t1", folder: "inbox", from: ALICE, from_username: "alice", to: [ME], subject: "Q3 board minutes", preview: "Attached the minutes from Tuesday. Two open points on the budget…", created_at_ms: now - 3.6e6, received_at_ms: now - 3.5e6, read: false, starred: true, attachments: 2, labels: ["work"], external: false, bcc_copy: false, outgoing: false },
  { id: "m2", thread_id: "t2", folder: "inbox", from: BOB, from_username: "bob", to: [ME], subject: "Re: Lighthouse restoration", preview: "Looks good. I can be there Saturday morning if the tide allows.", created_at_ms: now - 8.6e7, received_at_ms: now - 8.6e7, read: true, starred: false, attachments: 0, labels: [], external: false, bcc_copy: false, outgoing: false },
  { id: "m3", thread_id: "t3", folder: "inbox", from: CAROL, from_username: "", to: [ME], subject: "Invoice 2026-091", preview: "Please find the invoice for the September batch. Payment within 14 days.", created_at_ms: now - 2 * 8.6e7, received_at_ms: now - 2 * 8.6e7, read: true, starred: false, attachments: 1, labels: ["invoice"], external: true, bcc_copy: false, outgoing: false },
  { id: "m4", thread_id: "t4", folder: "requests", from: "hash1stranger0000000000000000000000000000", from_username: "", to: [ME], subject: "Hello from a stranger", preview: "We met at the conference; would love to follow up on the storage market idea.", created_at_ms: now - 4e7, received_at_ms: now - 4e7, read: false, starred: false, attachments: 0, labels: [], external: false, bcc_copy: false, outgoing: false },
  { id: "m5", thread_id: "t2", folder: "sent", from: ME, from_username: "me", to: [BOB], subject: "Lighthouse restoration", preview: "Bob, the blueprint is in Drive (live). Can you check the stairwell dimensions?", created_at_ms: now - 9e7, received_at_ms: now - 9e7, read: true, starred: false, attachments: 1, labels: [], external: false, bcc_copy: false, outgoing: true },
];

const mailView = (m: (typeof mails)[number]) => ({
  id: m.id,
  thread_id: m.thread_id,
  folder: m.folder,
  from: { address: m.from, username: m.from_username, display_name: "" },
  authenticated_sender: m.from,
  sender_matches: true,
  to: m.to.map((a) => ({ address: a, username: a === ME ? "me" : "", display_name: "" })),
  cc: [],
  bcc_copy: m.bcc_copy,
  created_at_ms: m.created_at_ms,
  received_at_ms: m.received_at_ms,
  subject: m.subject,
  body_text: `${m.preview}\n\nMore detail follows in plain text. Links become clickable: https://example.org/docs and nothing else is interpreted.\n\n— ${m.from_username || "sender"}`,
  body_html: m.external ? `<p><b>HTML version</b> of the invoice with an <img src="http://tracker.example/pixel.png"> that will not load.</p>` : "",
  attachments: Array.from({ length: m.attachments }, (_, i) => ({ index: i, name: i === 0 ? "minutes.pdf" : "blueprint.dwg", mime: i === 0 ? "application/pdf" : "application/octet-stream", size: 48_213 * (i + 1), kind: i === 0 ? "blob" : "drive", live: i === 1, share_id: i === 1 ? "s1" : "", version_no: i === 1 ? 2 : 0, folder: false, content_id: "", plaintext_hash: "ab".repeat(32) })),
  in_reply_to: "",
  references: [],
  external: m.external ? { gateway: "hash1gateway000000000000000000000000000000", from_header: "billing@vendor.example", message_id_header: "<x@vendor.example>", auth_results: ["spf=pass", "dkim=pass", "dmarc=pass"], spam_score: 12 } : null,
  request_read_receipt: false,
  importance: 0,
  sender_labels: [],
  labels: m.labels,
  read: m.read,
  starred: m.starred,
  delivered_to: m.outgoing ? { [BOB]: now - 8.9e7 } : {},
  read_by: m.outgoing ? { [BOB]: now - 8.8e7 } : {},
  outgoing: m.outgoing,
  trust_score: m.folder === "requests" ? -25 : 10,
  expire_after_secs: 0,
});

const entries = [
  { id: "f1", parent_id: "", kind: "folder", name: "Contracts", mime: "", size: 0, created_at_ms: now - 9e8, modified_at_ms: now - 8.6e7, versions: 0, trashed: false, starred: true, path: "/Contracts" },
  { id: "f2", parent_id: "", kind: "folder", name: "Photos", mime: "", size: 0, created_at_ms: now - 9e8, modified_at_ms: now - 2e8, versions: 0, trashed: false, starred: false, path: "/Photos" },
  { id: "e1", parent_id: "", kind: "file", name: "blueprint.dwg", mime: "application/octet-stream", size: 2_400_000, created_at_ms: now - 8e8, modified_at_ms: now - 3e6, versions: 1, trashed: false, starred: false, path: "/blueprint.dwg" },
  { id: "e2", parent_id: "", kind: "file", name: "notes.txt", mime: "text/plain", size: 1_204, created_at_ms: now - 7e8, modified_at_ms: now - 7e8, versions: 0, trashed: false, starred: false, path: "/notes.txt" },
  { id: "e3", parent_id: "f1", kind: "file", name: "lease-2026.pdf", mime: "application/pdf", size: 312_000, created_at_ms: now - 6e8, modified_at_ms: now - 6e8, versions: 3, trashed: false, starred: true, path: "/Contracts/lease-2026.pdf" },
];

const contacts = [
  { address: ALICE, username: "alice", display_name: "Alice Berg", states: ["friend", "trusted"], updated_at_ms: now - 1e8 },
  { address: BOB, username: "bob", display_name: "", states: ["friend", "following"], updated_at_ms: now - 2e8 },
  { address: CAROL, username: "carol", display_name: "Carol", states: ["pending_in"], updated_at_ms: now - 3e6 },
];

const mode = new URLSearchParams(location.search).get("shim") ?? "app";
const theme = new URLSearchParams(location.search).get("theme") ?? "dark";

const handlers: Record<string, (a: Args) => unknown> = {
  onboarding_generate: () => "abandon ability able about above absent absorb abstract absurd abuse access accident account accuse achieve acid acoustic acquire across act action actor actress actual".split(" "),
  onboarding_check_words: () => true,
  onboarding_create: () => ({ address: ME, device_id: "home-pc-1a2b", has_wallet_key: true, has_root_key: true }),
  identity_register: () => ({ hash: "AB".repeat(32), summary: "Create identity" }),
  unlock: () => ({ address: ME, device_id: "home-pc-1a2b", has_wallet_key: true, has_root_key: true }),
  lock: () => undefined,
  wipe_local_data: () => undefined,
  app_status: () => ({ vault_exists: mode !== "onboarding", unlocked: mode === "app", address: ME, network: "mainnet", chain_id: "hashgram-1", genesis_hash: "e322bc2319f6e0173286fa526dab5a8ff8ad0797c7b80dd03e7c9d98621d5e4d", onboarding_done: true, hello_available: true, hello_enabled: false, version: "0.3.0-dev", commit: "devshim", data_dir: "C:\\Users\\you\\AppData\\Local\\Hashgram\\data", updater_configured: false, updater_endpoint: "", code_signed: false, uptime_ms: 1200, link_up: true, link_error: null }),
  settings_get: () => ({ network: { kind: "mainnet", devnet_genesis_hash: "", bootstrap: [], chain_api: "", indexer_url: "", gateway_address: "" }, security: { auto_lock_minutes: 15, hello_enabled: false, clipboard_clear_secs: 30 }, appearance: { theme, reduced_motion: false, density: "comfortable", language: new URLSearchParams(location.search).get("lang") ?? "en" }, notifications: { mail: true, requests: true, spaces: true, circles: true }, mail: { threaded: true, mark_read_after_secs: 0 }, updates: { auto_check: false, channel: "stable" }, advanced: { log_level: "info" }, start_with_windows: false, onboarding_done: true, device_label: "Home PC" }),
  sync_status: () => ({ phase: "Idle", last_ok_ms: now - 12_000, rounds: 42, last_error: null, peers: 1, balance_uhash: "12500000" }),
  mail_counts: () => ({ inbox: { total: 3, unread: 1 }, requests: { total: 1, unread: 1 }, sent: { total: 1, unread: 0 }, drafts: { total: 0, unread: 0 }, archive: { total: 0, unread: 0 }, spam: { total: 0, unread: 0 }, trash: { total: 0, unread: 0 } }),
  mail_list: (a) => mails.filter((m) => (a?.folder === "starred" ? m.starred : m.folder === a?.folder)),
  mail_get: (a) => { const m = mails.find((x) => x.id === a?.id); return m ? mailView(m) : null; },
  mail_thread: (a) => { const ms = mails.filter((m) => m.thread_id === a?.threadId).sort((x, y) => x.created_at_ms - y.created_at_ms); return ms.length ? { id: a?.threadId, subject: ms[0]!.subject, messages: ms.map(mailView), participants: [...new Set(ms.flatMap((m) => [m.from, ...m.to]))], unread: ms.filter((m) => !m.read).length } : null; },
  mail_search: (a) => mails.filter((m) => m.subject.toLowerCase().includes(String(a?.q).toLowerCase())),
  mail_live_attachment_versions: () => ({ s1: 3 }),
  mail_draft_list: () => [],
  mail_draft_new: () => ({ id: "d1", to: [], cc: [], bcc: [], subject: "", body_text: "", body_html: "", attachments: [], in_reply_to: "", updated_at_ms: now }),
  mail_draft_save: (a) => ({ id: "d1", ...(a?.fields as object), attachments: [], in_reply_to: "", updated_at_ms: now }),
  mail_draft_get: () => ({ id: "d1", to: [], cc: [], bcc: [], subject: "", body_text: "", body_html: "", attachments: [], in_reply_to: "", updated_at_ms: now }),
  mail_resolve_recipients: (a) => (a?.inputs as string[]).map((input) => (input.includes("@") && !input.startsWith("@") && !input.endsWith("@hashgram.io") ? { input, kind: "external", address: input, username: "", display_name: "", has_identity: false, devices: 0, error: "external e-mail needs a mail gateway; set one in Settings → Network" } : { input, kind: "hashgram", address: ALICE, username: input.replace(/^@/, "").replace(/@hashgram\.io$/, ""), display_name: "", has_identity: true, devices: 2, error: "" })),
  mail_mark_read: () => undefined,
  mail_star: () => undefined,
  mail_archive: () => undefined,
  mail_trash: () => undefined,
  mail_accept_request: () => undefined,
  mail_settings_get: () => ({ send_read_receipts: false, keep_sent: true, purge_after_days: 30 }),
  people_username_of: (a) => ({ [ALICE]: "alice", [BOB]: "bob", [CAROL]: "carol", [ME]: "me" })[String(a?.address)] ?? "",
  people_list: (a) => (a?.which === "incoming" ? contacts.filter((c) => c.states.includes("pending_in")) : a?.which === "friends" ? contacts.filter((c) => c.states.includes("friend")) : a?.which === "following" ? contacts.filter((c) => c.states.includes("following")) : a?.which === "blocked" ? [] : contacts),
  people_search_local: (a) => contacts.filter((c) => c.username.includes(String(a?.q)) || c.display_name.toLowerCase().includes(String(a?.q).toLowerCase())),
  people_profile: (a) => { const c = contacts.find((x) => x.address === a?.address); return { address: a?.address, username: c?.username ?? "", display_name: c?.display_name ?? "", bio: c ? "Building things on the Hashgram network." : "", avatar_cid: "", states: c?.states ?? [] }; },
  people_resolve: (a) => ({ address: ALICE, username: String(a?.input).replace(/^@/, ""), display_name: "", mail_address: `${String(a?.input).replace(/^@/, "")}@hashgram.io`, has_identity: true, devices: 2 }),
  people_card_of: () => null,
  people_my_display_name: () => "Me",
  feed_author: () => [],
  feed_friends: () => [{ id: "p1", kind: "POST_CREATE", author: ALICE, timestamp: Math.floor(now / 1000) - 3600, payload: { text: "Shipped the storage lease client model today. Verify-before-pay, memo payments, no escrow needed until v1.1.", hashtags: ["hashgram", "storage"] }, media: [], visibility: "public" }, { id: "p2", kind: "POST_CREATE", author: BOB, timestamp: Math.floor(now / 1000) - 86400, payload: { text: "Lighthouse restoration weekend is on. Bring gloves." }, media: [], visibility: "public" }],
  feed_following: () => [],
  feed_thread: (a) => ({ post: { id: a?.post, kind: "POST_CREATE", author: ALICE, timestamp: Math.floor(now / 1000) - 3600, payload: { text: "Shipped the storage lease client model today." }, media: [], visibility: "public" }, comments: [{ id: "c1", kind: "COMMENT_CREATE", author: BOB, timestamp: Math.floor(now / 1000) - 3000, payload: { text: "Nice." }, media: [], visibility: "public" }], reactions: { "❤": 3 } }),
  // Hashwall / Explore / profile
  hashwall_explore: (a) => ({ items: Number(a?.before ?? 0) ? [] : [{ id: "e1", kind: "POST_CREATE", author: BOB, timestamp: Math.floor(now / 1000) - 600, payload: { text: "Protest at Rustaveli on Saturday, 17:00. Bring water.", hashtags: ["tbilisi", "protest"] }, media: [], visibility: "public", channel: "ab".repeat(32), reply_to: "" }, { id: "e2", kind: "POST_CREATE", author: ALICE, timestamp: Math.floor(now / 1000) - 7200, payload: { text: "Looking for Rust developers for a P2P storage project.", hashtags: ["rust", "jobs"] }, media: [], visibility: "public", channel: "", reply_to: "" }], next_before: 0, source_peer: "12D3KooWDev", source_operator: "hash1operator", source_rtt_ms: 42, note: "" }),
  hashwall_digest: () => ({ window_secs: 604800, events: 2, authors: 2, total_events: 40, total_authors: 9, top_authors: [{ author: ALICE, posts: 12, comments: 4, reactions_received: 30, comments_received: 6, last_active: Math.floor(now / 1000) - 7200 }, { author: BOB, posts: 5, comments: 9, reactions_received: 8, comments_received: 2, last_active: Math.floor(now / 1000) - 600 }], top_hashtags: [{ tag: "tbilisi", posts: 7, authors: 4, last_used: Math.floor(now / 1000) - 600 }, { tag: "rust", posts: 3, authors: 2, last_used: Math.floor(now / 1000) - 7200 }], walls: [{ id: "ab".repeat(32), name: "ArchevnebiSaqartveloshi", description: "Elections in Georgia — reports from polling stations.", creator: ALICE, open_posting: true, created_at: Math.floor(now / 1000) - 86400, posts: 7, authors: 4, last_post: Math.floor(now / 1000) - 600, pinned: true }], computed_at: Math.floor(now / 1000), source_peer: "12D3KooWDev", source_operator: "hash1operator", source_rtt_ms: 42, note: "" }),
  hashwall_thread: (a) => handlers.feed_thread!(a),
  walls_create: (a) => ({ id: "cd".repeat(32), name: a?.name, description: a?.description ?? "", creator: ME, open_posting: !!a?.openPosting, created_at: Math.floor(now / 1000), posts: 0, authors: 0, last_post: 0, pinned: true }),
  walls_info: (a) => ({ id: a?.wall, name: "ArchevnebiSaqartveloshi", description: "Elections in Georgia — reports from polling stations.", creator: ALICE, open_posting: true, created_at: Math.floor(now / 1000) - 86400, posts: 7, authors: 4, last_post: Math.floor(now / 1000) - 600, pinned: true }),
  walls_page: (a) => handlers.hashwall_explore!(a),
  walls_pin: () => handlers.walls_pinned!({}),
  walls_pinned: () => [{ id: "ab".repeat(32), name: "ArchevnebiSaqartveloshi", description: "Elections in Georgia — reports from polling stations.", creator: ALICE, open_posting: true, created_at: Math.floor(now / 1000) - 86400, posts: 7, authors: 4, last_post: Math.floor(now / 1000) - 600, pinned: true }],
  people_profile_cached: (a) => handlers.people_profile!(a),
  people_avatar: () => null,
  profile_me: () => ({ address: ME, username: "me", display_name: "Me", bio: "Testing Hashgram One.", avatar_cid: "", balance: handlers.wallet_balance!({}), activity: { posts: 3, comments: 5, reactions: 12, reposts: 1, walls_created: 1, following: 2, events: 24, walls_posted: ["ab".repeat(32)], first_event: Math.floor(now / 1000) - 30 * 86400, last_event: Math.floor(now / 1000) - 600, reactions_received: 9, comments_received: 2, score: 3 * 10 + 5 * 3 + 2 + 15 + 12 + 18 + 8 }, refreshed: true, walls: handlers.walls_pinned!({}), friends: 2, following: 2 }),
  profile_my_events: () => [{ id: "m1", kind: "POST_CREATE", author: ME, timestamp: Math.floor(now / 1000) - 600, payload: { text: "Hello from my PC.", hashtags: [] }, media: [], visibility: "public", channel: "", reply_to: "" }, { id: "m2", kind: "REACTION", author: ME, timestamp: Math.floor(now / 1000) - 900, payload: { post_id: "p1", reaction: "❤" }, media: [], visibility: "public", channel: "", reply_to: "" }],
  network_holders: () => ({ holders: [{ rank: 1, address: ALICE, balance_uhash: "9000000000", share_bps: 6000, username: "alice" }, { rank: 2, address: BOB, balance_uhash: "4000000000", share_bps: 2666, username: "" }, { rank: 3, address: ME, balance_uhash: "2000000000", share_bps: 1333, username: "me" }], accounts_scanned: 3, complete: true, total_uhash: "15000000000", height: 12345 }),
  network_providers: () => [],
  circles_list: () => [{ id: "c1", name: "Family", description: "", members: [ME, ALICE, BOB], since: 1, owner: true }],
  circles_merged: () => [{ circle: "c1", item: { id: "cp1", kind: "post", author: ALICE, at_ms: now - 5e6, text: "Sunday lunch — where?", post_id: "", reply_to: "", media: [], drive_refs: [], poll: { question: "Where?", options: ["Pizza", "Sushi", "Home"], multiple_choice: false, closes_at_ms: 0 }, reactions: { "❤": 2 }, votes: { "0": 2, "2": 1 }, deleted: false } }],
  circles_comments: () => [],
  spaces_list: () => [{ id: "sp1", name: "Project X", description: "the deal", my_role: 4, members: 3, group_id: "g", created_at_ms: now - 9e8 }, { id: "sp2", name: "Family", description: "", my_role: 2, members: 5, group_id: "g2", created_at_ms: now - 9e9 }],
  spaces_state: (a) => ({ space_id: a?.space, name: a?.space === "sp1" ? "Project X" : "Family", description: a?.space === "sp1" ? "the deal" : "", members: [{ address: ME, role: a?.space === "sp1" ? 4 : 2, since_ms: now - 9e8 }, { address: ALICE, role: 3, since_ms: now - 8e8 }, { address: BOB, role: 1, since_ms: now - 7e8 }], my_role: a?.space === "sp1" ? 4 : 2, drive: [{ capability: { share_id: "sh1", owner: ALICE, entry_id: "e9", name: "term-sheet.pdf", mime: "application/pdf", size: 120_000, mode: "live", permission: "read", version_no: 4, granted_at_ms: now - 3e8, folder: false, plaintext_hash: "" }, path: "Contracts/2026", by: ALICE, at_ms: now - 3e8 }], created_at_ms: now - 9e8, content_count: 2 }),
  spaces_content: () => [{ id: "a1", kind: "announcement", actor: ME, at_ms: now - 2e8, title: "Kick-off Monday", text: "10:00, room 4. Bring the term sheet.", post_id: "", drive_refs: [], media: [] }, { id: "po1", kind: "post", actor: ALICE, at_ms: now - 1e8, title: "", text: "Draft two is in the Space drive.", post_id: "", drive_refs: [], media: [] }],
  drive_list: (a) => entries.filter((e) => e.parent_id === (a?.parent ?? "")),
  drive_entry: (a) => entries.find((e) => e.id === a?.id) ?? null,
  drive_starred: () => entries.filter((e) => e.starred),
  drive_trash_list: () => [],
  drive_search: (a) => entries.filter((e) => e.name.includes(String(a?.q))),
  drive_usage: () => ({ files: 3, folders: 2, trashed: 0, bytes: 2_713_204, revision: 12, committed_revision: 11, dirty: true }),
  drive_shared_with_me: () => [{ capability: { share_id: "s1", owner: BOB, entry_id: "x", name: "stairwell.dwg", mime: "application/octet-stream", size: 900_000, mode: "live", permission: "read", version_no: 3, granted_at_ms: now - 5e8, folder: false, plaintext_hash: "" }, from: BOB, group_id: "g", note: "latest survey", received_at_ms: now - 5e8, updates: 1, revoked: false }],
  drive_shares: () => [{ share_id: "sh9", entry_id: "e1", grantee: BOB, mode: "live", permission: "read", granted_at_ms: now - 9e7, revoked: false, revoked_at_ms: 0 }],
  drive_versions: () => [{ version_no: 1, size: 2_100_000, created_at_ms: now - 8e8, device_pubkey: "aa", note: "" }, { version_no: 2, size: 2_400_000, created_at_ms: now - 3e6, device_pubkey: "aa", note: "current" }],
  wallet_overview: () => ({ address: ME, balance: { address: ME, uhash: "12500000", display: "12.500000 HASH", verification: "verified by 1 node — only one operator reachable" }, account_exists: true, account_number: 7, sequence: 3, vesting_type: null, original_vesting_uhash: null, vesting_end: null, vesting_start: null, username: "me", can_sign: true }),
  wallet_balance: () => ({ address: ME, uhash: "12500000", display: "12.500000 HASH", verification: "verified by 1 node — only one operator reachable" }),
  tx_recent: () => [{ hash: "AB".repeat(32), submitted: Math.floor(now / 1000) - 5000, summary: "Register @me", state: "committed", height: 91234, raw_log: "" }],
  wallet_history: () => null,
  network_validators: () => [{ operator_address: "hashvaloper1abc000000000000000000000000000000000000", description: { moniker: "genesis" }, tokens: "500000000000000", status: "BOND_STATUS_BONDED", jailed: false, commission: { commission_rates: { rate: "0.05" } } }],
  wallet_delegations: () => ({ delegation_responses: [] }),
  wallet_rewards: () => ({ rewards: [] }),
  wallet_unbonding: () => ({ unbonding_responses: [] }),
  wallet_usernames: () => ({ registrations: [{ name: "me", owner: ME, expiry_height: "7884000" }] }),
  wallet_username_availability: (a) => ({ available: String(a?.name) !== "alice", normalized: String(a?.name), reason: String(a?.name) === "alice" ? "taken" : "", conflicting_name: "" }),
  identity_status: () => {
    const ready = !new URLSearchParams(window.location.search).has("unregistered");
    return { address: ME, registered: ready, devices: ready ? [{ device_id: "home-pc-1a2b", label: "Home PC", platform: "windows", revoked: false, pubkey_hex: "cd".repeat(32), is_this_device: true }, { device_id: "laptop-9f", label: "Laptop", platform: "windows", revoked: false, pubkey_hex: "ef".repeat(32), is_this_device: false }] : [], this_device_id: "home-pc-1a2b", this_device_pubkey: "cd".repeat(32), this_device_registered: ready, rotation_count: ready ? 0 : null, username: ready ? "me" : "", balance_uhash: ready ? "12500000" : "0", has_wallet_key: true, has_root_key: true, online: true };
  },
  qr_svg: () => `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 21 21"><rect width="21" height="21" fill="#ffffff"/><path d="M1 1h7v7h-7zM13 1h7v7h-7zM1 13h7v7h-7zM10 10h1v1h-1zM12 12h2v2h-2z" fill="#000000"/></svg>`,
  network_overview: () => ({ network_id: "hashgram-mainnet", chain_id: "hashgram-1", genesis_hash: "e322bc2319f6e0173286fa526dab5a8ff8ad0797c7b80dd03e7c9d98621d5e4d", peers: [{ peer: "12D3KooWGenesisNode000000000000000000000000000", roles: ["bootstrap", "relay", "store", "media"], operator: "hash1op000000000000000000000000000000000000" }], rejected: [["12D3KooWStranger", "wrong network (different genesis)"]], height: 912_340, verification: "single operator", operators: 1, store_peers: 1, relay_peers: 1, link_up: true, link_error: null, indexer_configured: false }),
  network_supply: () => ({ supply: { amount: { denom: "uhash", amount: "1000000000000000" } }, service_reserve: null, founder_revenue: null }),
  earn_status: () => ({ status: { operator: ME, reward_address: "", roles: [], bond_uhash: "0", declared_storage_bytes: 0, fraud_score: 0, jailed: false, jailed_until_height: 0, unbonding_height: 0, registered_height: 0, moniker: "", lifecycle: "Unregistered", raw: {} }, sentence: "Not registered as a provider. Run a node and register to earn from the storage and relay work it does." }),
  earn_earnings: () => ({ total_paid_uhash: "0", pending_credit: "0", epoch: 42, reserve_remaining_uhash: "300000000000000", raw: {} }),
  earn_providers: () => [],
  node_overview: () => ({ bundled: false, configured: false, setup: null, registration: "none", running: false, operator: null, operator_balance_uhash: null, status: null, rewards: null, provider: null, reachability: "unknown", elevated: false, chain_gateway: "http://127.0.0.1:26680", binary: null, node_id: null }),
  help_list: () => [{ slug: "quick-start", title: "Quick start", docs_path: "" }, { slug: "mail", title: "Mail", docs_path: "" }],
  help_page: () => "<h1>Quick start</h1><p>Rendered by the Rust side in the real app.</p>",
  about_info: () => ({ version: "0.3.0-dev", commit: "devshim", genesis_hash: "e322bc2319f6e0173286fa526dab5a8ff8ad0797c7b80dd03e7c9d98621d5e4d", chain_id: "hashgram-1", kdf: "Argon2id, 64 MiB", code_signed: false, updater_endpoint: "", data_dir: "C:\\…\\Hashgram\\data", logs_dir: "C:\\…\\Hashgram\\data\\logs", licenses: [["Hashgram", "Apache-2.0"], ["Inter (font)", "SIL OFL 1.1"]] }),
  perf_mark: () => undefined,
  perf_snapshot: () => [],
  perf_memory: () => 142 * 1024 * 1024,
  touch: () => undefined,
  ui_log: () => undefined,
  search_recent: () => [],
  search_note: () => undefined,
  sync_now: () => undefined,
  leases_list: () => [],
  tx_has_pending: () => false,
};

export function installDevShim() {
  const w = window as unknown as { __TAURI_INTERNALS__?: unknown; __HASHGRAM_DEVSHIM__?: boolean };
  if (w.__TAURI_INTERNALS__ || !import.meta.env.DEV) return;
  w.__HASHGRAM_DEVSHIM__ = true;
  w.__TAURI_INTERNALS__ = {
    invoke: async (cmd: string, args?: Args) => {
      const h = handlers[cmd];
      if (!h) throw { code: "internal", message: `devshim: no handler for ${cmd}`, retryable: false };
      await new Promise((r) => setTimeout(r, 30));
      return h(args);
    },
    transformCallback: (cb: (...a: unknown[]) => void) => {
      const id = Math.floor(Math.random() * 1e9);
      (window as unknown as Record<string, unknown>)[`_${id}`] = cb;
      return id;
    },
    metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main" } },
    convertFileSrc: (p: string) => p,
  };
  // The event plugin calls `plugin:event|listen` through invoke; answer it.
  handlers["plugin:event|listen"] = () => 1;
  handlers["plugin:event|unlisten"] = () => undefined;
  handlers["plugin:dialog|open"] = () => null;
  handlers["plugin:dialog|save"] = () => null;
  handlers["plugin:dialog|ask"] = () => true;
  handlers["plugin:clipboard-manager|write_text"] = () => undefined;
  handlers["plugin:opener|open_url"] = () => undefined;
  handlers["plugin:autostart|enable"] = () => undefined;
  handlers["plugin:autostart|disable"] = () => undefined;
  console.info("[hashgram] dev shim active: fake data, no Tauri");
}
