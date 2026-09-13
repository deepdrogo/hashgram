// Typed wrappers over the Rust commands. Every type here mirrors a Rust
// struct in src-tauri/src/{views,cmd_*}.rs; keep them in step. Nothing here
// ever carries key material: the Rust side maps records through views.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

export interface UiError {
  code: string;
  message: string;
  retryable: boolean;
}

export function isUiError(e: unknown): e is UiError {
  return !!e && typeof e === "object" && "code" in e && "message" in e;
}

/** The UiError inside `e`, looking through Error.cause chains (SolidJS
 *  `createResource` wraps non-Error rejections in an Error whose `cause`
 *  is the original value). */
export function uiError(e: unknown): UiError | null {
  let cur: unknown = e;
  for (let depth = 0; depth < 5 && cur !== undefined && cur !== null; depth++) {
    if (isUiError(cur)) return cur;
    if (cur instanceof Error && "cause" in cur) {
      cur = (cur as Error & { cause?: unknown }).cause;
      continue;
    }
    break;
  }
  return null;
}

export function errText(e: unknown): string {
  const u = uiError(e);
  if (u) return u.message;
  if (e instanceof Error) return e.message;
  if (typeof e === "string") return e;
  if (e && typeof e === "object" && "message" in e && typeof (e as { message: unknown }).message === "string") {
    return (e as { message: string }).message;
  }
  return String(e);
}

export function errCode(e: unknown): string {
  return uiError(e)?.code ?? "internal";
}

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    if (isUiError(e)) throw e;
    throw { code: "internal", message: String(e), retryable: false } satisfies UiError;
  }
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export type NetworkKind = "mainnet" | "devnet";

export interface AppStatus {
  vault_exists: boolean;
  unlocked: boolean;
  address: string | null;
  network: NetworkKind;
  chain_id: string;
  genesis_hash: string;
  onboarding_done: boolean;
  hello_available: boolean;
  hello_enabled: boolean;
  version: string;
  commit: string;
  data_dir: string;
  updater_configured: boolean;
  updater_endpoint: string;
  code_signed: boolean;
  uptime_ms: number;
  link_up: boolean;
  link_error: string | null;
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
    bootstrap: string[];
    chain_api: string;
    indexer_url: string;
    gateway_address: string;
  };
  security: { auto_lock_minutes: number; hello_enabled: boolean; clipboard_clear_secs: number };
  appearance: { theme: "dark" | "light" | "system"; reduced_motion: boolean; density: "comfortable" | "compact"; language: "en" | "ka" };
  notifications: { mail: boolean; requests: boolean; spaces: boolean; circles: boolean };
  mail: { threaded: boolean; mark_read_after_secs: number };
  updates: { auto_check: boolean; channel: string };
  advanced: { log_level: string };
  start_with_windows: boolean;
  onboarding_done: boolean;
  device_label: string;
}

export type SyncPhase =
  | "Offline"
  | "Connecting"
  | "Discovering"
  | "Idle"
  | { Syncing: "Mailbox" | "Outbox" | "Feed" | "Wallet" | "Devices" }
  | { Backoff: { attempt: number; wait_secs: number } };

export interface SyncStatus {
  phase: SyncPhase;
  last_ok_ms: number | null;
  rounds: number;
  last_error: string | null;
  peers: number;
  balance_uhash: string | null;
}

export type SyncEvent =
  | { kind: "phase"; phase: SyncPhase }
  | { kind: "new_mail"; id: string; folder: string }
  | { kind: "drive_share_changed" }
  | { kind: "contacts_changed" }
  | { kind: "circle_activity"; id: string }
  | { kind: "space_activity"; id: string }
  | { kind: "balance"; uhash: string }
  | { kind: "unsupported_message" }
  | { kind: "warning"; message: string }
  | { kind: "round_done"; mail: number; drive: number; people: number; circles: number; spaces: number; feed: number; drive_committed: number | null; elapsed_ms: number };

// Mail
export interface FolderCounts {
  total: number;
  unread: number;
}
export interface MailSummary {
  id: string;
  thread_id: string;
  folder: string;
  from: string;
  from_username: string;
  to: string[];
  subject: string;
  preview: string;
  created_at_ms: number;
  received_at_ms: number;
  read: boolean;
  starred: boolean;
  attachments: number;
  labels: string[];
  external: boolean;
  bcc_copy: boolean;
  outgoing: boolean;
}
export interface AddressView {
  address: string;
  username: string;
  display_name: string;
}
export interface AttachmentView {
  index: number;
  name: string;
  mime: string;
  size: number;
  kind: "inline" | "blob" | "drive";
  live: boolean;
  share_id: string;
  version_no: number;
  folder: boolean;
  content_id: string;
  plaintext_hash: string;
}
export interface ExternalView {
  gateway: string;
  from_header: string;
  message_id_header: string;
  auth_results: string[];
  spam_score: number;
}
export interface MailView {
  id: string;
  thread_id: string;
  folder: string;
  from: AddressView;
  authenticated_sender: string;
  sender_matches: boolean;
  to: AddressView[];
  cc: AddressView[];
  bcc_copy: boolean;
  created_at_ms: number;
  received_at_ms: number;
  subject: string;
  body_text: string;
  body_html: string;
  attachments: AttachmentView[];
  in_reply_to: string;
  references: string[];
  external: ExternalView | null;
  request_read_receipt: boolean;
  importance: number;
  sender_labels: string[];
  labels: string[];
  read: boolean;
  starred: boolean;
  delivered_to: Record<string, number>;
  read_by: Record<string, number>;
  outgoing: boolean;
  trust_score: number;
  expire_after_secs: number;
}
export interface ThreadView {
  id: string;
  subject: string;
  messages: MailView[];
  participants: string[];
  unread: number;
}
export interface DraftView {
  id: string;
  to: string[];
  cc: string[];
  bcc: string[];
  subject: string;
  body_text: string;
  body_html: string;
  attachments: AttachmentView[];
  in_reply_to: string;
  updated_at_ms: number;
}
export interface DraftFields {
  to: string[];
  cc: string[];
  bcc: string[];
  subject: string;
  body_text: string;
  body_html: string;
}
export interface MailSettings {
  send_read_receipts: boolean;
  keep_sent: boolean;
  purge_after_days: number;
}
export interface RecipientResolution {
  input: string;
  kind: "hashgram" | "external" | "invalid";
  address: string;
  username: string;
  display_name: string;
  has_identity: boolean;
  devices: number;
  error: string;
}

// Drive
export interface EntryView {
  id: string;
  parent_id: string;
  kind: "file" | "folder";
  name: string;
  mime: string;
  size: number;
  created_at_ms: number;
  modified_at_ms: number;
  versions: number;
  trashed: boolean;
  starred: boolean;
  path: string;
}
export interface DriveUsage {
  files: number;
  folders: number;
  trashed: number;
  bytes: number;
  revision: number;
  committed_revision: number;
  dirty: boolean;
}
export interface CapabilityView {
  share_id: string;
  owner: string;
  entry_id: string;
  name: string;
  mime: string;
  size: number;
  mode: "snapshot" | "live";
  permission: "read" | "write";
  version_no: number;
  granted_at_ms: number;
  folder: boolean;
  plaintext_hash: string;
}
export interface SharedWithMeView {
  capability: CapabilityView;
  from: string;
  group_id: string;
  note: string;
  received_at_ms: number;
  updates: number;
  revoked: boolean;
}
export interface ShareRecordView {
  share_id: string;
  entry_id: string;
  grantee: string;
  mode: "snapshot" | "live";
  permission: "read" | "write";
  granted_at_ms: number;
  revoked: boolean;
  revoked_at_ms: number;
}
export interface VersionView {
  version_no: number;
  size: number;
  created_at_ms: number;
  device_pubkey: string;
  note: string;
  plaintext_hash: string;
}
export interface FolderEntryView {
  id: string;
  parent_id: string;
  kind: "file" | "folder";
  name: string;
  mime: string;
  size: number;
  modified_at_ms: number;
}
export interface DriveProgress {
  op: string;
  stage: "reading" | "encrypting" | "uploading" | "done" | "failed";
  done: number;
  total: number;
  message: string;
}

// People
export interface Resolved {
  address: string;
  username: string;
  display_name: string;
  mail_address: string;
  has_identity: boolean;
  devices: number;
}
export interface Profile {
  address: string;
  username: string;
  display_name: string;
  bio: string;
  avatar_cid: string;
  states: string[];
}
export interface ContactRecord {
  address: string;
  username: string;
  display_name: string;
  states: string[];
  updated_at_ms: number;
}
export interface CardView {
  display_name: string;
  bio: string;
  wallet_address: string;
  at_ms: number;
}

// Feed / circles
export interface FeedItem {
  id: string;
  kind: string;
  author: string;
  timestamp: number;
  payload: Record<string, unknown>;
  media: [string, string, number][];
  visibility: string;
  /** Wall (channel) hex id, "" for none. */
  channel: string;
  /** Post replied to, hex id, "" for none. */
  reply_to: string;
}
export interface PostThread {
  post: FeedItem;
  comments: FeedItem[];
  reactions: Record<string, number>;
}
/** One page of a remote timeline (Explore, wall, hashtag). */
export interface ExplorePage {
  items: FeedItem[];
  /** `before` for the next page; 0 when exhausted. */
  next_before: number;
  source_peer: string;
  source_operator: string;
  source_rtt_ms: number | null;
  /** Empty when a node answered; otherwise why this came from the local cache. */
  note: string;
}
export interface WallInfo {
  id: string;
  name: string;
  description: string;
  creator: string;
  open_posting: boolean;
  created_at: number;
  posts: number;
  authors: number;
  last_post: number;
  pinned: boolean;
}
export interface AuthorActivity {
  author: string;
  posts: number;
  comments: number;
  reactions_received: number;
  comments_received: number;
  last_active: number;
}
export interface HashtagActivity {
  tag: string;
  posts: number;
  authors: number;
  last_used: number;
}
export interface Digest {
  window_secs: number;
  events: number;
  authors: number;
  total_events: number;
  total_authors: number;
  top_authors: AuthorActivity[];
  top_hashtags: HashtagActivity[];
  walls: WallInfo[];
  computed_at: number;
  source_peer: string;
  source_operator: string;
  source_rtt_ms: number | null;
  /** Empty when a node answered; otherwise why this came from the local cache. */
  note: string;
}
export interface MyActivity {
  posts: number;
  comments: number;
  reactions: number;
  reposts: number;
  walls_created: number;
  following: number;
  events: number;
  walls_posted: string[];
  first_event: number;
  last_event: number;
  reactions_received: number;
  comments_received: number;
  score: number;
}
export interface MyProfile {
  address: string;
  username: string;
  display_name: string;
  bio: string;
  avatar_cid: string;
  balance: Balance | null;
  activity: MyActivity;
  refreshed: boolean;
  walls: WallInfo[];
  friends: number;
  following: number;
}
export interface Holder {
  rank: number;
  address: string;
  balance_uhash: string;
  share_bps: number;
  username: string;
}
export interface Holders {
  holders: Holder[];
  accounts_scanned: number;
  complete: boolean;
  total_uhash: string;
  height: number | null;
}
export interface CircleInfo {
  id: string;
  name: string;
  description: string;
  members: string[];
  since: number;
  owner: boolean;
}
export interface MediaView {
  cid: string;
  mime: string;
  size: number;
  name: string;
  kind: string;
  width: number;
  height: number;
}
export interface PollView {
  question: string;
  options: string[];
  multiple_choice: boolean;
  closes_at_ms: number;
}
export interface CircleItemView {
  id: string;
  kind: "post" | "comment";
  author: string;
  at_ms: number;
  text: string;
  post_id: string;
  reply_to: string;
  media: MediaView[];
  drive_refs: CapabilityView[];
  poll: PollView | null;
  reactions: Record<string, number>;
  votes: Record<string, number>;
  deleted: boolean;
}
export interface MergedItem {
  circle: string;
  item: CircleItemView;
}
export interface PollInput {
  question: string;
  options: string[];
  multiple_choice: boolean;
  closes_at_ms: number;
}

// Spaces
export interface SpaceSummary {
  id: string;
  name: string;
  description: string;
  my_role: number;
  members: number;
  group_id: string;
  created_at_ms: number;
}
export interface SpaceMember {
  address: string;
  role: number;
  since_ms: number;
}
export interface SpaceContentView {
  id: string;
  kind: "post" | "comment" | "announcement";
  actor: string;
  at_ms: number;
  title: string;
  text: string;
  post_id: string;
  drive_refs: CapabilityView[];
  media: MediaView[];
}
export interface SpaceSharedEntryView {
  capability: CapabilityView;
  path: string;
  by: string;
  at_ms: number;
}
export interface SpaceStateView {
  space_id: string;
  name: string;
  description: string;
  members: SpaceMember[];
  my_role: number;
  drive: SpaceSharedEntryView[];
  created_at_ms: number;
  content_count: number;
}
export const ROLE = { none: 0, guest: 1, member: 2, admin: 3, owner: 4 } as const;

// Earn
export type Lifecycle = "Unregistered" | "Registered" | "WaitingForAssignment" | "Active" | "Degraded" | "Jailed" | "Unbonding" | "Withdrawn";
export interface ProviderStatus {
  operator: string;
  reward_address: string;
  roles: string[];
  bond_uhash: string;
  declared_storage_bytes: number;
  fraud_score: number;
  jailed: boolean;
  jailed_until_height: number;
  unbonding_height: number;
  registered_height: number;
  moniker: string;
  lifecycle: Lifecycle;
  raw: unknown;
}
export interface EarnStatus {
  status: ProviderStatus;
  sentence: string;
}
export interface Earnings {
  total_paid_uhash: string;
  pending_credit: string;
  epoch: number;
  reserve_remaining_uhash: string;
  raw: unknown;
}
export interface NodeSetup {
  roles: string[];
  storage_gib: number;
  bandwidth_mbps: number;
  reward_address: string;
  moniker: string;
  auto_register: boolean;
}
export type Registration = "none" | "scheduled_task" | "service";
export interface NodeOverview {
  bundled: boolean;
  configured: boolean;
  setup: NodeSetup | null;
  registration: Registration;
  running: boolean;
  operator: string | null;
  operator_balance_uhash: string | null;
  status: Record<string, unknown> | null;
  rewards: Record<string, unknown> | null;
  provider: EarnStatus | null;
  reachability: string;
  elevated: boolean;
  chain_gateway: string;
  binary: string | null;
  node_id: string | null;
}
export interface ColdAddress {
  words: string[];
  address: string;
}

// Wallet
export interface Balance {
  address: string;
  uhash: string;
  display: string;
  verification: string | null;
}
export interface WalletOverview {
  address: string;
  balance: Balance;
  account_exists: boolean;
  account_number: number | null;
  sequence: number | null;
  vesting_type: string | null;
  original_vesting_uhash: string | null;
  vesting_end: number | null;
  vesting_start: number | null;
  username: string;
  can_sign: boolean;
}
export interface TxPreview {
  summary: string;
  warnings: string[];
  gas_limit: number;
  fee_uhash: string;
  fee_display: string;
  simulated: boolean;
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
export interface UsernameAvailability {
  available: boolean;
  normalized: string;
  reason: string;
  conflicting_name: string;
}

// Identity / devices
export interface DeviceInfo {
  device_id: string;
  label: string;
  platform: string;
  revoked: boolean;
  pubkey_hex: string;
  is_this_device: boolean;
}
export interface IdentityStatus {
  address: string;
  registered: boolean;
  devices: DeviceInfo[];
  this_device_id: string;
  this_device_pubkey: string;
  this_device_registered: boolean;
  rotation_count: number | null;
  username: string;
  balance_uhash: string | null;
  has_wallet_key: boolean;
  has_root_key: boolean;
  online: boolean;
}
export interface ReconcileReport {
  groups: number;
  removed: [string, string][];
  added: [string, string][];
  errors: [string, string][];
}
export interface BackupMeta {
  exported_at: number;
  address: string;
  partial: boolean;
  from_device: string;
}
export interface BackupInfo {
  m_cost_kib: number;
  t_cost: number;
  p_cost: number;
  bytes: number;
}

// Network
export interface PeerView {
  peer: string;
  roles: string[];
  operator: string;
  /** Measured ping round-trip in ms; null until measured. */
  rtt_ms: number | null;
}
export interface NetworkOverview {
  network_id: string;
  chain_id: string;
  genesis_hash: string;
  peers: PeerView[];
  rejected: [string, string][];
  height: number | null;
  verification: string | null;
  operators: number;
  store_peers: number;
  relay_peers: number;
  link_up: boolean;
  link_error: string | null;
  indexer_configured: boolean;
}

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
export interface AboutInfo {
  version: string;
  commit: string;
  genesis_hash: string;
  chain_id: string;
  kdf: string;
  code_signed: boolean;
  updater_endpoint: string;
  data_dir: string;
  logs_dir: string;
  licenses: [string, string][];
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

export const ipc = {
  // identity / vault
  appStatus: () => call<AppStatus>("app_status"),
  onboardingGenerate: () => call<string[]>("onboarding_generate"),
  onboardingCheckWords: (positions: number[], words: string[]) => call<boolean>("onboarding_check_words", { positions, words }),
  onboardingCreate: (passphrase: string, deviceLabel: string) => call<AccountInfo>("onboarding_create", { passphrase, deviceLabel }),
  onboardingRestorePreview: (mnemonic: string) => call<{ address: string }>("onboarding_restore_preview", { mnemonic }),
  onboardingRestore: (mnemonic: string, passphrase: string, deviceLabel: string) => call<AccountInfo>("onboarding_restore", { mnemonic, passphrase, deviceLabel }),
  backupInspect: (path: string) => call<BackupInfo>("backup_inspect", { path }),
  restoreFromBackup: (path: string, backupPassphrase: string, vaultPassphrase: string, deviceLabel: string) =>
    call<AccountInfo>("restore_from_backup", { path, backupPassphrase, vaultPassphrase, deviceLabel }),
  unlock: (passphrase: string) => call<AccountInfo>("unlock", { passphrase }),
  lock: () => call<void>("lock"),
  touch: () => call<void>("touch"),
  changePassphrase: (current: string, next: string) => call<void>("change_passphrase", { current, new: next }),
  helloEnable: (passphrase: string) => call<void>("hello_enable", { passphrase }),
  helloUnlock: () => call<AccountInfo>("hello_unlock"),
  helloDisable: () => call<void>("hello_disable"),
  wipeLocalData: (confirm: string) => call<void>("wipe_local_data", { confirm }),
  identityStatus: () => call<IdentityStatus>("identity_status"),
  identityRegister: (label: string) => call<TxSubmitted>("identity_register", { label }),
  devicesList: () => call<DeviceInfo[]>("devices_list"),
  thisDevice: () => call<[string, string]>("this_device"),
  deviceAdd: (deviceId: string, pubkeyHex: string, label: string) => call<TxSubmitted>("device_add", { deviceId, pubkeyHex, label }),
  deviceRevoke: (deviceId: string) => call<TxSubmitted>("device_revoke", { deviceId }),
  devicesReconcile: () => call<ReconcileReport>("devices_reconcile"),
  devicesBootstrap: () => call<boolean>("devices_bootstrap"),

  // mail
  mailCounts: () => call<Record<string, FolderCounts>>("mail_counts"),
  mailList: (folder: string, opts?: { beforeMs?: number; limit?: number; threaded?: boolean }) =>
    call<MailSummary[]>("mail_list", { folder, beforeMs: opts?.beforeMs, limit: opts?.limit, threaded: opts?.threaded }),
  mailThread: (threadId: string) => call<ThreadView | null>("mail_thread", { threadId }),
  mailGet: (id: string) => call<MailView | null>("mail_get", { id }),
  mailSearch: (q: string, limit?: number) => call<MailSummary[]>("mail_search", { q, limit }),
  mailMarkRead: (id: string, read: boolean) => call<void>("mail_mark_read", { id, read }),
  mailStar: (id: string, starred: boolean) => call<void>("mail_star", { id, starred }),
  mailMove: (id: string, folder: string) => call<void>("mail_move", { id, folder }),
  mailArchive: (id: string) => call<void>("mail_archive", { id }),
  mailTrash: (id: string) => call<void>("mail_trash", { id }),
  mailDelete: (id: string) => call<void>("mail_delete", { id }),
  mailAcceptRequest: (id: string) => call<void>("mail_accept_request", { id }),
  mailLabel: (id: string, label: string, on: boolean) => call<void>("mail_label", { id, label, on }),
  mailSettingsGet: () => call<MailSettings>("mail_settings_get"),
  mailSettingsSet: (settings: MailSettings) => call<void>("mail_settings_set", { settings }),
  mailPurge: () => call<number>("mail_purge"),
  mailResolveRecipients: (inputs: string[]) => call<RecipientResolution[]>("mail_resolve_recipients", { inputs }),
  mailDraftNew: (opts?: { replyTo?: string; all?: boolean; forward?: string }) =>
    call<DraftView>("mail_draft_new", { replyTo: opts?.replyTo, all: opts?.all, forward: opts?.forward }),
  mailDraftSave: (id: string, fields: DraftFields) => call<DraftView>("mail_draft_save", { id, fields }),
  mailDraftList: () => call<DraftView[]>("mail_draft_list"),
  mailDraftGet: (id: string) => call<DraftView>("mail_draft_get", { id }),
  mailDraftDelete: (id: string) => call<boolean>("mail_draft_delete", { id }),
  mailAttachFile: (draftId: string, path: string) => call<DraftView>("mail_attach_file", { draftId, path }),
  mailAttachBytes: (draftId: string, name: string, mime: string, base64: string) => call<DraftView>("mail_attach_bytes", { draftId, name, mime, base64 }),
  mailAttachDrive: (draftId: string, entryId: string, live: boolean) => call<DraftView>("mail_attach_drive", { draftId, entryId, live }),
  mailDraftRemoveAttachment: (draftId: string, index: number) => call<DraftView>("mail_draft_remove_attachment", { draftId, index }),
  mailSend: (draftId: string, fields: DraftFields, options?: { request_read_receipt?: boolean; importance?: number; expire_after_secs?: number }) =>
    call<string>("mail_send", { draftId, fields, options }),
  mailAttachmentSave: (id: string, index: number, path: string) => call<number>("mail_attachment_save", { id, index, path }),
  mailAttachmentOpen: (id: string, index: number) => call<string>("mail_attachment_open", { id, index }),
  mailAttachmentPreview: (id: string, index: number) => call<string>("mail_attachment_preview", { id, index }),
  mailLiveAttachmentVersions: () => call<Record<string, number>>("mail_live_attachment_versions"),

  // drive
  driveList: (parent: string) => call<EntryView[]>("drive_list", { parent }),
  driveTrashList: () => call<EntryView[]>("drive_trash_list"),
  driveStarred: () => call<EntryView[]>("drive_starred"),
  driveSearch: (q: string, limit?: number) => call<EntryView[]>("drive_search", { q, limit }),
  driveEntry: (id: string) => call<EntryView | null>("drive_entry", { id }),
  driveVersions: (id: string) => call<VersionView[]>("drive_versions", { id }),
  driveUsage: () => call<DriveUsage>("drive_usage"),
  driveResolvePath: (path: string) => call<string | null>("drive_resolve_path", { path }),
  driveMkdir: (parent: string, name: string) => call<string>("drive_mkdir", { parent, name }),
  driveUpload: (parent: string, path: string, op: string) => call<string>("drive_upload", { parent, path, op }),
  driveUploadBytes: (parent: string, name: string, mime: string, base64: string) => call<string>("drive_upload_bytes", { parent, name, mime, base64 }),
  driveUpdate: (id: string, path: string, note: string, op: string) => call<number>("drive_update", { id, path, note, op }),
  driveDownload: (id: string, outPath: string) => call<number>("drive_download", { id, outPath }),
  driveDownloadVersion: (id: string, versionNo: number, outPath: string) => call<number>("drive_download_version", { id, versionNo, outPath }),
  driveOpen: (id: string) => call<string>("drive_open", { id }),
  drivePreview: (id: string) => call<string>("drive_preview", { id }),
  driveRename: (id: string, name: string) => call<void>("drive_rename", { id, name }),
  driveMove: (id: string, newParent: string) => call<void>("drive_move", { id, newParent }),
  driveCopy: (id: string, newParent: string, name: string) => call<string>("drive_copy", { id, newParent, name }),
  driveTrash: (id: string) => call<void>("drive_trash", { id }),
  driveRestore: (id: string) => call<void>("drive_restore", { id }),
  driveDelete: (id: string) => call<number>("drive_delete", { id }),
  driveEmptyTrash: () => call<number>("drive_empty_trash"),
  driveStar: (id: string, on: boolean) => call<void>("drive_star", { id, on }),
  driveRestoreVersion: (id: string, versionNo: number) => call<void>("drive_restore_version", { id, versionNo }),
  driveRekey: (id: string) => call<number>("drive_rekey", { id }),
  driveRekeyAll: () => call<number>("drive_rekey_all"),
  driveCommit: () => call<number>("drive_commit"),
  driveShare: (id: string, grantees: string[], live: boolean, note: string) => call<CapabilityView>("drive_share", { id, grantees, live, note }),
  driveRevoke: (shareId: string) => call<void>("drive_revoke", { shareId }),
  driveShares: (entryId?: string) => call<ShareRecordView[]>("drive_shares", { entryId }),
  driveSharedWithMe: () => call<SharedWithMeView[]>("drive_shared_with_me"),
  driveSharedDownload: (shareId: string, outPath: string) => call<number>("drive_shared_download", { shareId, outPath }),
  driveSharedOpen: (shareId: string) => call<string>("drive_shared_open", { shareId }),
  driveSharedSave: (shareId: string, parent: string) => call<string>("drive_shared_save", { shareId, parent }),
  driveSharedFolderList: (shareId: string) => call<FolderEntryView[]>("drive_shared_folder_list", { shareId }),
  driveSharedFolderDownload: (shareId: string, entryId: string, outPath: string) => call<number>("drive_shared_folder_download", { shareId, entryId, outPath }),

  // people
  peopleResolve: (input: string) => call<Resolved>("people_resolve", { input }),
  peopleProfile: (address: string) => call<Profile>("people_profile", { address }),
  peopleRequest: (input: string, message: string) => call<Resolved>("people_request", { input, message }),
  peopleRespond: (address: string, accept: boolean) => call<void>("people_respond", { address, accept }),
  peopleRemove: (address: string) => call<void>("people_remove", { address }),
  peopleBlock: (address: string) => call<void>("people_block", { address }),
  peopleUnblock: (address: string) => call<void>("people_unblock", { address }),
  peopleMute: (address: string, on: boolean) => call<void>("people_mute", { address, on }),
  peopleTrust: (address: string, on: boolean) => call<void>("people_trust", { address, on }),
  peopleFollow: (address: string, on: boolean) => call<void>("people_follow", { address, on }),
  peopleList: (which: "friends" | "incoming" | "outgoing" | "blocked" | "all" | "following") => call<ContactRecord[]>("people_list", { which }),
  peopleSearchLocal: (q: string) => call<ContactRecord[]>("people_search_local", { q }),
  peopleSetDisplayName: (name: string) => call<void>("people_set_display_name", { name }),
  peopleMyDisplayName: () => call<string>("people_my_display_name"),
  peopleSendCard: (address: string, bio: string, discloseWallet: boolean) => call<void>("people_send_card", { address, bio, discloseWallet }),
  peopleCardOf: (address: string) => call<CardView | null>("people_card_of", { address }),
  peopleUsernameOf: (address: string) => call<string>("people_username_of", { address }),

  // feed / circles
  feedFollowing: (before?: number, limit?: number) => call<FeedItem[]>("feed_following", { before, limit }),
  feedFriends: (before?: number, limit?: number) => call<FeedItem[]>("feed_friends", { before, limit }),
  feedAuthor: (address: string, before?: number, limit?: number) => call<FeedItem[]>("feed_author", { address, before, limit }),
  feedExplore: (before?: number, limit?: number, tag?: string) => call<unknown | null>("feed_explore", { before, limit, tag }),
  feedThread: (post: string) => call<PostThread | null>("feed_thread", { post }),
  feedPost: (text: string, hashtags: string[], mediaPaths: string[], sensitive: boolean, channel?: string) =>
    call<string>("feed_post", { text, hashtags, mediaPaths, sensitive, channel }),
  feedComment: (post: string, text: string) => call<string>("feed_comment", { post, text }),
  feedReact: (target: string, reaction: string) => call<string>("feed_react", { target, reaction }),
  feedRepost: (post: string, comment: string) => call<string>("feed_repost", { post, comment }),
  feedEdit: (post: string, text: string) => call<string>("feed_edit", { post, text }),
  feedDelete: (post: string) => call<string>("feed_delete", { post }),
  feedRefresh: () => call<number>("feed_refresh"),
  feedFollows: () => call<string[]>("feed_follows"),
  feedProfileUpdate: (name: string, bio: string, avatarPath?: string) => call<string>("feed_profile_update", { name, bio, avatarPath }),
  feedMediaFetch: (cid: string, mime: string) => call<string>("feed_media_fetch", { cid, mime }),
  circlesList: () => call<CircleInfo[]>("circles_list"),
  circlesCreate: (name: string, description: string, members: string[]) => call<string>("circles_create", { name, description, members }),
  circlesAddMember: (circle: string, member: string) => call<void>("circles_add_member", { circle, member }),
  circlesRemoveMember: (circle: string, address: string) => call<number>("circles_remove_member", { circle, address }),
  circlesLeave: (circle: string) => call<void>("circles_leave", { circle }),
  circlesPost: (circle: string, text: string, mediaPaths: string[], poll?: PollInput) => call<string>("circles_post", { circle, text, mediaPaths, poll }),
  circlesComment: (circle: string, post: string, text: string) => call<string>("circles_comment", { circle, post, text }),
  circlesReact: (circle: string, target: string, reaction: string) => call<string>("circles_react", { circle, target, reaction }),
  circlesVote: (circle: string, post: string, choices: number[]) => call<string>("circles_vote", { circle, post, choices }),
  circlesDelete: (circle: string, target: string) => call<string>("circles_delete", { circle, target }),
  circlesSetInfo: (circle: string, name: string, description: string) => call<string>("circles_set_info", { circle, name, description }),
  circlesPosts: (circle: string, beforeMs?: number, limit?: number) => call<CircleItemView[]>("circles_posts", { circle, beforeMs, limit }),
  circlesComments: (circle: string, post: string) => call<CircleItemView[]>("circles_comments", { circle, post }),
  circlesMerged: (beforeMs?: number, limit?: number) => call<MergedItem[]>("circles_merged", { beforeMs, limit }),
  circlesMediaFetch: (circle: string, item: string, index: number) => call<string>("circles_media_fetch", { circle, item, index }),

  // hashwall: explore (P2P), walls, profile, avatars, rich list
  hashwallExplore: (before?: number, limit?: number, tag?: string) => call<ExplorePage>("hashwall_explore", { before, limit, tag }),
  hashwallDigest: (windowSecs?: number, limit?: number) => call<Digest>("hashwall_digest", { windowSecs, limit }),
  hashwallThread: (post: string) => call<PostThread | null>("hashwall_thread", { post }),
  wallsCreate: (name: string, description: string, openPosting: boolean) => call<WallInfo>("walls_create", { name, description, openPosting }),
  wallsInfo: (wall: string) => call<WallInfo>("walls_info", { wall }),
  wallsPage: (wall: string, before?: number, limit?: number) => call<ExplorePage>("walls_page", { wall, before, limit }),
  wallsPin: (wall: string, on: boolean) => call<WallInfo[]>("walls_pin", { wall, on }),
  wallsPinned: () => call<WallInfo[]>("walls_pinned"),
  peopleProfileCached: (address: string) => call<Profile>("people_profile_cached", { address }),
  peopleAvatar: (cid: string) => call<string>("people_avatar", { cid }),
  profileMe: () => call<MyProfile>("profile_me"),
  profileMyEvents: (before?: number, limit?: number) => call<FeedItem[]>("profile_my_events", { before, limit }),
  networkHolders: (limit?: number) => call<Holders>("network_holders", { limit }),
  networkProviders: () => call<ProviderStatus[]>("network_providers"),

  // spaces
  spacesList: () => call<SpaceSummary[]>("spaces_list"),
  spacesCreate: (name: string, description: string) => call<string>("spaces_create", { name, description }),
  spacesState: (space: string) => call<SpaceStateView>("spaces_state", { space }),
  spacesMembers: (space: string) => call<SpaceMember[]>("spaces_members", { space }),
  spacesContent: (space: string, beforeMs?: number, limit?: number) => call<SpaceContentView[]>("spaces_content", { space, beforeMs, limit }),
  spacesDrive: (space: string) => call<SpaceSharedEntryView[]>("spaces_drive", { space }),
  spacesInvite: (space: string, member: string, role: string) => call<void>("spaces_invite", { space, member, role }),
  spacesRemove: (space: string, address: string, reason: string) => call<void>("spaces_remove", { space, address, reason }),
  spacesSetRole: (space: string, address: string, role: string) => call<string>("spaces_set_role", { space, address, role }),
  spacesSetInfo: (space: string, name: string, description: string) => call<string>("spaces_set_info", { space, name, description }),
  spacesAnnounce: (space: string, title: string, text: string) => call<string>("spaces_announce", { space, title, text }),
  spacesPost: (space: string, text: string) => call<string>("spaces_post", { space, text }),
  spacesComment: (space: string, post: string, text: string) => call<string>("spaces_comment", { space, post, text }),
  spacesShareDrive: (space: string, entry: string, path: string, live: boolean) => call<string>("spaces_share_drive", { space, entry, path, live }),
  spacesUnshareDrive: (space: string, share: string) => call<string>("spaces_unshare_drive", { space, share }),
  spacesMail: (space: string, subject: string, body: string) => call<string>("spaces_mail", { space, subject, body }),
  spacesDriveDownload: (space: string, share: string, outPath: string) => call<number>("spaces_drive_download", { space, share, outPath }),
  spacesDriveOpen: (space: string, share: string) => call<string>("spaces_drive_open", { space, share }),
  spacesDriveSave: (space: string, share: string, parent: string) => call<string>("spaces_drive_save", { space, share, parent }),
  spacesDriveFolderList: (space: string, share: string) => call<FolderEntryView[]>("spaces_drive_folder_list", { space, share }),

  // earn
  earnStatus: (operator?: string) => call<EarnStatus>("earn_status", { operator }),
  earnEarnings: (operator?: string) => call<Earnings>("earn_earnings", { operator }),
  earnProviders: () => call<ProviderStatus[]>("earn_providers"),
  earnRegister: (rewardAddress: string, nodePubkeyHex: string, roles: string[], bond: string, storageGib: number, moniker: string) =>
    call<TxSubmitted>("earn_register", { rewardAddress, nodePubkeyHex, roles, bond, storageGib, moniker }),
  earnUpdate: (rewardAddress: string, roles: string[], storageGib: number, moniker: string, additionalBond: string) =>
    call<TxSubmitted>("earn_update", { rewardAddress, roles, storageGib, moniker, additionalBond }),
  earnUnbond: () => call<TxSubmitted>("earn_unbond"),
  earnWithdraw: () => call<TxSubmitted>("earn_withdraw"),
  nodeOverview: () => call<NodeOverview>("node_overview"),
  nodeConfigure: (setup: NodeSetup) => call<string>("node_configure", { setup }),
  nodeInstall: () => call<Registration>("node_install"),
  nodeStart: () => call<void>("node_start"),
  nodeStop: () => call<void>("node_stop"),
  nodeUninstall: () => call<void>("node_uninstall"),
  nodeGenerateColdAddress: () => call<ColdAddress>("node_generate_cold_address"),
  nodeLogTail: (lines?: number) => call<string>("node_log_tail", { lines }),

  // wallet
  walletBalance: (address?: string) => call<Balance>("wallet_balance", { address }),
  walletOverview: () => call<WalletOverview>("wallet_overview"),
  txPreview: (spec: MsgSpec) => call<TxPreview>("tx_preview", { spec }),
  txSubmit: (spec: MsgSpec, memo: string) => call<TxSubmitted>("tx_submit", { spec, memo }),
  txRecent: () => call<PendingRow[]>("tx_recent"),
  txHasPending: () => call<boolean>("tx_has_pending"),
  walletParseAmount: (amount: string) => call<string>("wallet_parse_amount", { amount }),
  walletPreviewSend: (to: string, amount: string) => call<TxPreview>("wallet_preview_send", { to, amount }),
  walletSend: (to: string, amount: string, memo: string) => call<TxSubmitted>("wallet_send", { to, amount, memo }),
  walletStake: (validator: string, amount: string) => call<TxSubmitted>("wallet_stake", { validator, amount }),
  walletUnstake: (validator: string, amount: string) => call<TxSubmitted>("wallet_unstake", { validator, amount }),
  walletWithdrawRewards: (validator: string) => call<TxSubmitted>("wallet_withdraw_rewards", { validator }),
  walletUsernameAvailability: (name: string) => call<UsernameAvailability>("wallet_username_availability", { name }),
  walletRegisterUsername: (name: string) => call<TxSubmitted>("wallet_register_username", { name }),
  walletRenewUsername: (name: string) => call<TxSubmitted>("wallet_renew_username", { name }),
  walletDelegations: () => call<Record<string, unknown>>("wallet_delegations"),
  walletRewards: () => call<Record<string, unknown>>("wallet_rewards"),
  walletUnbonding: () => call<Record<string, unknown>>("wallet_unbonding"),
  walletHistory: (limit?: number) => call<Record<string, unknown>>("wallet_history", { limit }),
  walletTx: (hash: string) => call<Record<string, unknown> | null>("wallet_tx", { hash }),
  walletUsernames: () => call<Record<string, unknown>>("wallet_usernames"),
  chainQuery: (path: string) => call<unknown>("chain_query", { path }),
  qrSvg: (text: string) => call<string>("qr_svg", { text }),

  // network
  networkOverview: () => call<NetworkOverview>("network_overview"),
  networkValidators: () => call<Record<string, unknown>[]>("network_validators"),
  networkSupply: () => call<Record<string, unknown>>("network_supply"),
  networkTop: (what: "holders" | "validators" | "providers" | "earners", limit?: number) => call<unknown | null>("network_top", { what, limit }),
  networkStats: () => call<unknown | null>("network_stats"),
  netReconnect: () => call<void>("net_reconnect"),
  netForgetPeers: () => call<void>("net_forget_peers"),
  diagnosticsExport: () => call<string>("diagnostics_export"),

  // sync
  syncStatus: () => call<SyncStatus>("sync_status"),
  syncNow: () => call<void>("sync_now"),

  // settings, backup, misc
  settingsGet: () => call<Settings>("settings_get"),
  settingsSet: (settings: Settings) => call<void>("settings_set", { settings }),
  backupExport: (path: string, passphrase: string) => call<BackupMeta>("backup_export", { path, passphrase }),
  helpList: () => call<HelpPage[]>("help_list"),
  helpPage: (slug: string) => call<string>("help_page", { slug }),
  perfSnapshot: () => call<SpanRecord[]>("perf_snapshot"),
  perfMark: (name: string, micros: number) => call<void>("perf_mark", { name, micros }),
  perfMemory: () => call<number>("perf_memory"),
  openDataDir: () => call<void>("open_data_dir"),
  uiLog: (level: "info" | "warn" | "error", message: string) => call<void>("ui_log", { level, message }),
  saveTextFile: (path: string, contents: string) => call<void>("save_text_file", { path, contents }),
  searchRecent: () => call<string[]>("search_recent"),
  searchNote: (query: string) => call<void>("search_note", { query }),
  aboutInfo: () => call<AboutInfo>("about_info"),
  leasesList: () => call<unknown[]>("leases_list"),
  leasesVerify: (leaseId: string, epoch: number) => call<unknown>("leases_verify", { leaseId, epoch }),
  windowHide: () => call<void>("window_hide"),
};

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

export interface EventMap {
  "sync:phase": SyncPhase;
  "sync:event": SyncEvent;
  "sync:tick": null;
  "session:locked": null;
  "session:unlocked": null;
  "settings:changed": null;
  "net:changed": null;
  "tx:update": { hash: string; state: string; height?: number; raw_log?: string };
  "drive:progress": DriveProgress;
  "deep-link": { url: string };
}

export function on<K extends keyof EventMap>(name: K, handler: (payload: EventMap[K]) => void): Promise<UnlistenFn> {
  return listen<EventMap[K]>(name, (e) => handler(e.payload));
}
