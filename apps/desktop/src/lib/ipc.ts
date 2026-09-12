// Typed wrappers over the Rust commands. Every type here mirrors a Rust
// struct in src-tauri/src/commands.rs; keep them in step.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type NetworkKind = "mainnet" | "devnet";

export interface AppStatus {
  vault_exists: boolean;
  unlocked: boolean;
  address: string | null;
  network: NetworkKind;
  chain_id: string;
  onboarding_done: boolean;
  hello_available: boolean;
  hello_enabled: boolean;
  version: string;
  commit: string;
  data_dir: string;
  updater_configured: boolean;
  updater_key_id: string;
  updater_endpoint: string;
  uptime_ms: number;
}

export interface AccountInfo {
  address: string;
  device_id: string;
  has_wallet_key: boolean;
  has_root_key: boolean;
}

export interface Settings {
  network: {
    kind: NetworkKind;
    devnet_genesis_hash: string;
    devnet_bootstrap: string[];
    https_endpoints: string[];
    local_node_api: string;
  };
  security: { auto_lock_minutes: number; hello_enabled: boolean; clipboard_clear_secs: number };
  appearance: { reduced_motion: boolean; compact: boolean };
  notifications: { messages: boolean; calls: boolean };
  media: { autoplay: boolean; cache_mb: number };
  updates: { auto_check: boolean; channel: string };
  advanced: { log_level: string };
  start_with_windows: boolean;
  onboarding_done: boolean;
  device_label: string;
}

export interface Verification {
  peers: string[];
  operators: string[];
  heights: number[];
  agreed: boolean;
  single_operator: boolean;
  disputed: string[];
}

export type Source =
  | { kind: "local_node"; url: string }
  | { kind: "p2p_relay" }
  | { kind: "https"; url: string };

export interface ChainRead {
  value: unknown;
  verification: Verification | null;
  source: Source;
  height: number;
  cached: boolean;
}

export interface EndpointHealth {
  source: Source;
  ok: boolean;
  detail: string;
  active: boolean;
  latency_ms: number;
}

export interface ChainHealth {
  sources: EndpointHealth[];
  active: Source | null;
  relay_operators: number;
}

export interface PeerView {
  peer_id: string;
  operator: string;
  roles: string[];
  latency_ms: number | null;
  transport: string;
  direction: string;
  connected_secs: number;
  discovery: string;
  served_last_read: boolean;
  agreed: boolean | null;
  verified: boolean;
  relays_chain: boolean;
}

export interface RejectedView {
  peer_id: string;
  reason: string;
  label: string;
  at: number;
}

export interface NetSnapshot {
  running: boolean;
  network: string;
  chain_id: string;
  genesis_hash: string;
  own_peer_id: string;
  peers: PeerView[];
  rejected: RejectedView[];
  nat: string;
  peerstore_size: number;
  builtin_seeds: string[];
  kad_peers: number;
  verified: number;
  last_read: Verification | null;
  uptime_secs: number;
  listen_addrs: string[];
  external_addrs: string[];
}

export interface WalletOverview {
  address: string;
  balance_uhash: string;
  account_exists: boolean;
  account_number: number | null;
  sequence: number | null;
  vesting_type: string | null;
  original_vesting_uhash: string | null;
  vesting_end: number | null;
  vesting_start: number | null;
  username: string | null;
  verification: Verification | null;
  source: Source;
  height: number;
}

export type MsgSpec =
  | { type: "send"; to: string; amount_uhash: string }
  | { type: "delegate"; validator: string; amount_uhash: string }
  | { type: "undelegate"; validator: string; amount_uhash: string }
  | { type: "redelegate"; from_validator: string; to_validator: string; amount_uhash: string }
  | { type: "withdraw_rewards"; validator: string }
  | { type: "vote"; proposal_id: number; option: string }
  | { type: "register_username"; name: string }
  | { type: "renew_username"; name: string }
  | { type: "transfer_username"; name: string; to: string }
  | { type: "release_username"; name: string }
  | { type: "set_recovery_config"; guardians: string[]; threshold: number; delay_blocks: number }
  | { type: "cancel_recovery" }
  | { type: "revoke_device"; device_id: string }
  | { type: "begin_unbonding" }
  | { type: "withdraw_bond" };

export interface TxPreview {
  summary: string;
  warnings: string[];
  gas_limit: number;
  fee_uhash: string;
  simulated: boolean;
  founder_share_uhash: string;
  source: Source;
}

export interface TxSubmitted {
  hash: string;
  summary: string;
}

export interface PendingRow {
  hash: string;
  submitted: number;
  summary: string;
  state: "pending" | "committed" | "failed";
  height: number;
  raw_log: string;
}

export interface DeviceInfo {
  device_id: string;
  label: string;
  platform: string;
  revoked: boolean;
  pubkey_hex: string;
  is_this_device: boolean;
}

export interface IdentityStatus {
  registered: boolean;
  devices: DeviceInfo[];
  this_device_id: string;
  this_device_pubkey: string;
  this_device_registered: boolean;
  rotation_count: number | null;
  recovery: unknown;
}

export type SearchResult =
  | { kind: "address"; address: string; username: string | null }
  | { kind: "username"; name: string; address: string; expiry_height: number | null }
  | { kind: "username_available"; name: string; confusable_with: string[] }
  | { kind: "tx"; hash: string; height: number | null; found: boolean }
  | { kind: "hashtag"; tag: string }
  | { kind: "channel"; id: string }
  | { kind: "nothing"; reason: string };

export interface HelpPage {
  slug: string;
  title: string;
  docs_path: string;
}

export interface SpanRecord {
  name: string;
  micros: number;
  at_ms: number;
  origin: "rust" | "ui";
}

// ---- Stage 2: messages, social, calls ------------------------------------

export interface ConversationMeta {
  group_id: string;
  name: string;
  direct: boolean;
  members: string[];
  last_preview: string;
  last_ts: number;
  disappear_secs: number;
  unread: number;
}

export interface AttachmentView {
  cid: string;
  key: string;
  nonce: string;
  mime: string;
  size: number;
  name: string;
  kind: string;
  width: number;
  height: number;
  duration_ms: number;
  plaintext_hash: string;
}

export interface CallSignalView {
  kind: string;
  call_id: string;
  sdp: string;
  candidate: string;
  sdp_mid: string;
  sdp_mline_index: number;
  video: boolean;
}

export interface MessageView {
  id: string;
  group_id: string;
  kind: string;
  sender: string;
  sender_device: string;
  outgoing: boolean;
  this_device: boolean;
  timestamp_ms: number;
  text: string;
  reply_to: string;
  target: string;
  reaction: string;
  attachments: AttachmentView[];
  disappear_after_secs: number;
  state: string;
  expires: number;
  call_kind: string;
  call: CallSignalView | null;
  reactions: Record<string, string[]>;
  group_name: string;
}

export interface ChatInfo {
  members: [string, string, string][];
  store_nodes: string[];
  disappear_secs: number;
  last_sync_secs: number | null;
}

export interface MediaView {
  cid: string;
  mime: string;
  size: number;
  kind: string;
  width: number;
  height: number;
  duration_ms: number;
  content_hash: string;
}

export interface EventView {
  id: string;
  kind: string;
  author: string;
  username: string | null;
  display_name: string | null;
  sequence: number;
  timestamp: number;
  payload: Record<string, unknown>;
  media: MediaView[];
  device: string;
  reactions: Record<string, number>;
  comments: number;
  reposts: number;
  my_reaction: string | null;
}

export interface FeedPage {
  events: EventView[];
  hidden: number;
  authors: string[];
}

export interface ProfileView {
  address: string;
  profile: Record<string, unknown> | null;
  following: boolean;
  block_mode: string | null;
  events: number;
}

export interface NodeSetup {
  roles: string[];
  storage_gib: number;
  bandwidth_mbps: number;
  reward_address: string;
  moniker: string;
  auto_register: boolean;
}

export interface NodeOverview {
  bundled: boolean;
  configured: boolean;
  setup: NodeSetup | null;
  registration: "none" | "scheduled_task" | "service";
  running: boolean;
  operator: string | null;
  operator_balance_uhash: string | null;
  status: Record<string, unknown> | null;
  rewards: Record<string, unknown> | null;
  provider: Record<string, unknown> | null;
  assignments: Record<string, unknown> | null;
  challenges: Record<string, unknown> | null;
  fraud: Record<string, unknown> | null;
  chain_rewards: Record<string, unknown> | null;
  reachability: string;
  elevated: boolean;
  chain_gateway: string;
  binary: string | null;
}

export interface CallInfra {
  nodes: Record<string, unknown>[];
  sfu_available: boolean;
}

export interface IceServer {
  urls: string[];
  username: string;
  credential: string;
  expires_at: number;
}

const call = <T,>(cmd: string, args?: Record<string, unknown>) => invoke<T>(cmd, args);

export const ipc = {
  appStatus: () => call<AppStatus>("app_status"),
  onboardingGenerate: () => call<string[]>("onboarding_generate"),
  onboardingCheckWords: (positions: number[], words: string[]) =>
    call<boolean>("onboarding_check_words", { positions, words }),
  onboardingCreate: (passphrase: string, deviceLabel: string) =>
    call<AccountInfo>("onboarding_create", { passphrase, deviceLabel }),
  onboardingRestorePreview: (mnemonic: string) =>
    call<{ address: string }>("onboarding_restore_preview", { mnemonic }),
  onboardingRestore: (mnemonic: string, passphrase: string, deviceLabel: string) =>
    call<AccountInfo>("onboarding_restore", { mnemonic, passphrase, deviceLabel }),
  unlock: (passphrase: string) => call<AccountInfo>("unlock", { passphrase }),
  lock: () => call<void>("lock"),
  touch: () => call<void>("touch"),
  changePassphrase: (current: string, next: string) =>
    call<void>("change_passphrase", { current, new: next }),
  helloEnable: (passphrase: string) => call<void>("hello_enable", { passphrase }),
  helloUnlock: () => call<AccountInfo>("hello_unlock"),
  helloDisable: () => call<void>("hello_disable"),
  wipeLocalData: (confirm: string) => call<void>("wipe_local_data", { confirm }),
  settingsGet: () => call<Settings>("settings_get"),
  settingsSet: (settings: Settings) => call<void>("settings_set", { settings }),
  netSnapshot: () => call<NetSnapshot>("net_snapshot"),
  netMeasureLatency: () => call<void>("net_measure_latency"),
  netForgetPeers: () => call<void>("net_forget_peers"),
  netReconnect: () => call<void>("net_reconnect"),
  chainHealth: (probe: boolean) => call<ChainHealth>("chain_health", { probe }),
  diagnosticsExport: () => call<string>("diagnostics_export"),
  chainGet: (path: string) => call<ChainRead>("chain_get", { path }),
  chainGetMany: (paths: string[]) =>
    call<Array<{ Ok: ChainRead } | { Err: string }>>("chain_get_many", { paths }),
  walletOverview: () => call<WalletOverview>("wallet_overview"),
  txPreview: (spec: MsgSpec) => call<TxPreview>("tx_preview", { spec }),
  txSubmit: (spec: MsgSpec, memo = "") => call<TxSubmitted>("tx_submit", { spec, memo }),
  txRecent: () => call<PendingRow[]>("tx_recent"),
  txStatus: (hash: string) => call<unknown>("tx_status", { hash }),
  txHasPending: () => call<boolean>("tx_has_pending"),
  identityStatus: () => call<IdentityStatus>("identity_status"),
  identityRegister: (label: string) => call<TxSubmitted>("identity_register", { label }),
  searchResolve: (query: string) => call<SearchResult>("search_resolve", { query }),
  searchRecent: () => call<string[]>("search_recent"),
  qrSvg: (text: string) => call<string>("qr_svg", { text }),
  helpList: () => call<HelpPage[]>("help_list"),
  helpPage: (slug: string) => call<string>("help_page", { slug }),
  perfSnapshot: () => call<SpanRecord[]>("perf_snapshot"),
  perfMark: (name: string, micros: number) => call<void>("perf_mark", { name, micros }),
  perfMemory: () => call<number>("perf_memory"),
  openDataDir: () => call<void>("open_data_dir"),
  saveTextFile: (path: string, contents: string) => call<void>("save_text_file", { path, contents }),

  // Messages
  chatList: () => call<ConversationMeta[]>("chat_list"),
  chatHistory: (groupId: string, beforeTs?: number, limit?: number) => call<MessageView[]>("chat_history", { groupId, beforeTs, limit }),
  chatStartDirect: (address: string) => call<string>("chat_start_direct", { address }),
  chatCreateGroup: (name: string, members: string[]) => call<string>("chat_create_group", { name, members }),
  chatAddMember: (groupId: string, address: string) => call<void>("chat_add_member", { groupId, address }),
  chatRemoveMember: (groupId: string, address: string) => call<number>("chat_remove_member", { groupId, address }),
  chatSendText: (groupId: string, text: string, replyTo?: string) => call<MessageView>("chat_send_text", { groupId, text, replyTo }),
  chatSendFile: (groupId: string, path: string, caption?: string) => call<MessageView>("chat_send_file", { groupId, path, caption }),
  chatSendVoice: (groupId: string, audioBase64: string, durationMs: number) => call<MessageView>("chat_send_voice", { groupId, audioBase64, durationMs }),
  chatAttachment: (attachment: AttachmentView) => call<string>("chat_attachment", { attachment }),
  chatReact: (groupId: string, target: string, reaction: string) => call<void>("chat_react", { groupId, target, reaction }),
  chatEdit: (groupId: string, target: string, text: string) => call<void>("chat_edit", { groupId, target, text }),
  chatDelete: (groupId: string, target: string) => call<void>("chat_delete", { groupId, target }),
  chatMarkRead: (groupId: string) => call<void>("chat_mark_read", { groupId }),
  chatTyping: (groupId: string) => call<void>("chat_typing", { groupId }),
  chatTypingIn: (groupId: string) => call<string[]>("chat_typing_in", { groupId }),
  chatSetDisappear: (groupId: string, secs: number) => call<void>("chat_set_disappear", { groupId, secs }),
  chatInfo: (groupId: string) => call<ChatInfo>("chat_info", { groupId }),
  chatSearch: (query: string) => call<MessageView[]>("chat_search", { query }),
  chatSyncNow: () => call<number>("chat_sync_now"),
  chatPublishKeyPackages: () => call<number>("chat_publish_key_packages"),

  // Social
  feed: (kinds?: string[], tag?: string, beforeTs?: number, limit?: number) => call<FeedPage>("feed", { kinds, tag, beforeTs, limit }),
  feedRefresh: () => call<number>("feed_refresh"),
  postCreate: (text: string, hashtags: string[], channel?: string, replyTo?: string, media?: MediaView[]) =>
    call<EventView>("post_create", { text, hashtags, channel, replyTo, media }),
  commentCreate: (post: string, text: string) => call<EventView>("comment_create", { post, text }),
  socialReact: (target: string, reaction: string) => call<void>("social_react", { target, reaction }),
  repost: (post: string, comment?: string) => call<void>("repost", { post, comment }),
  follow: (address: string, on: boolean) => call<void>("follow", { address, on }),
  follows: () => call<string[]>("follows"),
  socialBlock: (address: string, mode: string | null) => call<void>("social_block", { address, mode }),
  socialBlocks: () => call<[string, string][]>("social_blocks"),
  profileGet: (address: string, refresh: boolean) => call<ProfileView>("profile_get", { address, refresh }),
  profileUpdate: (displayName: string, bio: string, avatar?: MediaView) => call<EventView>("profile_update", { displayName, bio, avatar }),
  postThread: (id: string) => call<[EventView | null, EventView[]]>("post_thread", { id }),
  mediaUpload: (path: string, durationMs?: number) => call<MediaView>("media_upload", { path, durationMs }),
  mediaFetch: (media: MediaView) => call<string>("media_fetch", { media }),
  reelPublish: (media: MediaView, caption: string, hashtags: string[]) => call<EventView>("reel_publish", { media, caption, hashtags }),
  storyPublish: (media: MediaView, caption: string) => call<EventView>("story_publish", { media, caption }),
  channelCreate: (name: string, description: string, openPosting: boolean) => call<EventView>("channel_create", { name, description, openPosting }),
  channels: () => call<EventView[]>("channels"),
  channelPosts: (channelId: string) => call<EventView[]>("channel_posts", { channelId }),
  safetyVerdict: (subject: string) => call<unknown>("safety_verdict", { subject }),

  // Node (Earn)
  nodeOverview: () => call<NodeOverview>("node_overview"),
  nodeConfigure: (setup: NodeSetup) => call<string>("node_configure", { setup }),
  nodeInstall: () => call<string>("node_install"),
  nodeStart: () => call<void>("node_start"),
  nodeStop: () => call<void>("node_stop"),
  nodeUninstall: () => call<void>("node_uninstall"),
  nodeGenerateColdAddress: () => call<{ words: string[]; address: string }>("node_generate_cold_address"),
  nodeLogTail: (lines?: number) => call<string>("node_log_tail", { lines }),

  // Calls
  callsDiscover: () => call<CallInfra>("calls_discover"),
  callsTurn: () => call<IceServer>("calls_turn"),
  callsSignal: (groupId: string, kind: string, callId: string, opts: { sdp?: string; candidate?: string; sdpMid?: string; sdpMlineIndex?: number; video?: boolean } = {}) =>
    call<void>("calls_signal", { groupId, kind, callId, sdp: opts.sdp, candidate: opts.candidate, sdpMid: opts.sdpMid, sdpMlineIndex: opts.sdpMlineIndex, video: opts.video ?? false }),
  callsSignals: (groupId: string, sinceMs: number) => call<MessageView[]>("calls_signals", { groupId, sinceMs }),
};

export type Events = {
  "net:changed": void;
  "session:locked": void;
  "settings:changed": void;
  "tx:update": { hash: string; state: string; height?: number; raw_log?: string };
  "deep-link": { url: string };
  "chat:changed": { group_id?: string; new?: number; expired?: number };
  "chat:unread": number;
  "feed:changed": number;
};

export function on<K extends keyof Events>(
  name: K,
  handler: (payload: Events[K]) => void,
): Promise<UnlistenFn> {
  return listen<Events[K]>(name, (e) => handler(e.payload));
}

/** Reads a JSON path like "balance.amount" out of an unknown value. */
export function pick(value: unknown, path: string): unknown {
  let cur: unknown = value;
  for (const key of path.split(".")) {
    if (cur === null || cur === undefined) return undefined;
    if (typeof cur !== "object") return undefined;
    cur = (cur as Record<string, unknown>)[key];
  }
  return cur;
}

export const str = (v: unknown, fallback = ""): string =>
  typeof v === "string" ? v : typeof v === "number" ? String(v) : fallback;

export const num = (v: unknown, fallback = 0): number => {
  if (typeof v === "number") return v;
  if (typeof v === "string") {
    const n = Number(v);
    return Number.isFinite(n) ? n : fallback;
  }
  return fallback;
};

export const arr = (v: unknown): unknown[] => (Array.isArray(v) ? v : []);

/**
 * Amount of `denom` in a gateway value that may be a coin list
 * (`[{denom, amount}]`), a single coin, or a bare string. "0" when absent.
 */
export function coin(v: unknown, denom = "uhash"): string {
  if (Array.isArray(v)) {
    const c = v.find((x) => str(pick(x, "denom")) === denom);
    return c ? str(pick(c, "amount"), "0") : "0";
  }
  if (v && typeof v === "object") return str(pick(v, "amount"), "0");
  if (typeof v === "string") return v || "0";
  return "0";
}
