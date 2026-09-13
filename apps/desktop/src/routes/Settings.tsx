// Settings: Network, Security (passphrase, Hello, auto-lock, backup),
// Devices (link to Wallet), Notifications, Mail, Appearance, Updates,
// Advanced (leases, rekey all, wipe), About (version, genesis, signed or
// unsigned preview, licenses).
import { For, Show, createEffect, createResource, createSignal } from "solid-js";
import { useNavigate, useParams } from "@solidjs/router";
import { Fingerprint, KeyRound, Download, Upload, Trash2, RefreshCw, FolderOpen, ShieldCheck, ShieldAlert, Smartphone } from "lucide-solid";
import { Button, Card, Field, Input, Notice, Switch, Select, Tabs, Badge, Textarea } from "~/components/ui";
import { Mono } from "~/components/identity";
import { ipc, errText, type Settings, type MailSettings } from "~/lib/ipc";
import { store } from "~/lib/store";
import { t, LOCALES } from "~/lib/i18n";
import { updates } from "~/lib/updates";
import { formatBytes, formatMs } from "~/lib/format";
import { pickSavePath, confirm } from "~/lib/dialogs";
import { enable as autostartEnable, disable as autostartDisable } from "@tauri-apps/plugin-autostart";

type Tab = "network" | "security" | "devices" | "notifications" | "mail" | "appearance" | "updates" | "advanced" | "about";

export function SettingsRoute() {
  const params = useParams<{ tab?: string }>();
  const navigate = useNavigate();
  const tab = (): Tab => (params.tab as Tab) || "network";
  const [draft, setDraft] = createSignal<Settings | null>(null);
  createEffect(() => {
    const s = store.settings();
    if (s && !draft()) setDraft(structuredClone(s));
  });
  const patch = (f: (s: Settings) => void) => {
    const s = structuredClone(draft() ?? store.settings()!);
    f(s);
    setDraft(s);
    // Appearance applies live; everything else on Save.
    store.applyAppearance(s);
  };
  const [saving, setSaving] = createSignal(false);
  const dirty = () => JSON.stringify(draft()) !== JSON.stringify(store.settings());
  const save = async () => {
    const s = draft();
    if (!s) return;
    setSaving(true);
    try {
      await ipc.settingsSet(s);
      if (s.start_with_windows) await autostartEnable().catch(() => undefined);
      else await autostartDisable().catch(() => undefined);
      await store.refreshSettings();
      setDraft(structuredClone(store.settings()!));
      store.toast("Settings saved");
    } catch (e) {
      store.toast(errText(e), "error");
    } finally {
      setSaving(false);
    }
  };
  return (
    <div class="flex h-full flex-col">
      <div class="flex items-center gap-3 border-b border-border px-4">
        <Tabs
          class="flex-1 border-b-0"
          value={tab()}
          onChange={(v) => navigate(`/settings/${v}`)}
          tabs={[
            { id: "network", label: t("settings_network") },
            { id: "security", label: t("settings_security") },
            { id: "devices", label: t("settings_devices") },
            { id: "notifications", label: t("settings_notifications") },
            { id: "mail", label: t("settings_mail") },
            { id: "appearance", label: t("settings_appearance") },
            { id: "updates", label: t("settings_updates") },
            { id: "advanced", label: t("settings_advanced") },
            { id: "about", label: t("settings_about") },
          ]}
        />
        <Show when={dirty()}>
          <Button size="sm" onClick={save} loading={saving()}>
            {t("save")}
          </Button>
        </Show>
      </div>
      <div class="min-h-0 flex-1 overflow-auto">
        <div class="page max-w-3xl">
          <Show when={draft()}>
            {(s) => (
              <>
                <Show when={tab() === "network"}>
                  <div class="flex flex-col gap-4">
                    <Card title="Public indexer (optional)">
                      <div class="p-4">
                        <Field label="Indexer URL" hint="Serves Explore, leaderboards and network stats. A read model, never an authority; data is labelled as coming from it. Leave empty for none.">
                          <Input mono value={s().network.indexer_url} onInput={(e) => patch((x) => (x.network.indexer_url = e.currentTarget.value.trim()))} placeholder="https://…" />
                        </Field>
                      </div>
                    </Card>
                    <Card title="External e-mail gateway (optional)">
                      <div class="p-4">
                        <Field label="Gateway identity" hint="The @name or hash1… address of a mail gateway. Mail to plain e-mail addresses is sent to it with an ext-to label and leaves the network in the clear from there. Empty = external sending disabled.">
                          <Input mono value={s().network.gateway_address} onInput={(e) => patch((x) => (x.network.gateway_address = e.currentTarget.value.trim()))} placeholder="@gateway" />
                        </Field>
                      </div>
                    </Card>
                    <Card title="Network profile">
                      <div class="flex flex-col gap-3 p-4">
                        <Field label="Network">
                          <Select value={s().network.kind} onChange={(v) => patch((x) => (x.network.kind = v as "mainnet" | "devnet"))} options={[{ value: "mainnet", label: "Hashgram Mainnet" }, { value: "devnet", label: "Devnet (development only)" }]} />
                        </Field>
                        <Show when={s().network.kind === "devnet"}>
                          <Notice strong>DEVNET: nothing here has value. A persistent banner is shown while this is selected.</Notice>
                          <Field label="Devnet genesis hash (64 hex)">
                            <Input mono value={s().network.devnet_genesis_hash} onInput={(e) => patch((x) => (x.network.devnet_genesis_hash = e.currentTarget.value.trim()))} />
                          </Field>
                          <Field label="Chain REST gateway (devnet only)" hint="On Mainnet chain reads go through the P2P relay; leave empty.">
                            <Input mono value={s().network.chain_api} onInput={(e) => patch((x) => (x.network.chain_api = e.currentTarget.value.trim()))} placeholder="http://127.0.0.1:31417" />
                          </Field>
                        </Show>
                        <Field label="Bootstrap peers override" hint="One multiaddr per line. Empty on Mainnet means the list compiled into the app. Changing the profile locks the app and reconnects.">
                          <Textarea mono rows={3} value={s().network.bootstrap.join("\n")} onInput={(e) => patch((x) => (x.network.bootstrap = e.currentTarget.value.split(/\r?\n/).map((l) => l.trim()).filter(Boolean)))} />
                        </Field>
                      </div>
                    </Card>
                  </div>
                </Show>
                <Show when={tab() === "security"}>
                  <SecurityTab s={s()} patch={patch} />
                </Show>
                <Show when={tab() === "devices"}>
                  <Card title={t("settings_devices")}>
                    <div class="flex flex-col gap-2 p-4 text-[13px]">
                      <p>Devices are managed in Wallet → Devices: add a second device by pasting its key, revoke a lost one, reconcile encryption groups with the chain.</p>
                      <Button variant="secondary" size="sm" class="w-fit" onClick={() => navigate("/wallet/devices")}>
                        <Smartphone size={12} /> Open Wallet → Devices
                      </Button>
                      <Field label="This device's label" hint="Shown to your other devices and used when registering the device key.">
                        <Input value={s().device_label} onInput={(e) => patch((x) => (x.device_label = e.currentTarget.value))} />
                      </Field>
                    </div>
                  </Card>
                </Show>
                <Show when={tab() === "notifications"}>
                  <Card title={t("settings_notifications")}>
                    <div class="p-4">
                      <Switch label="New mail" hint="A toast saying who wrote — never the subject." checked={s().notifications.mail} onChange={(v) => patch((x) => (x.notifications.mail = v))} />
                      <Switch label="Requests" hint="First messages from strangers and contact requests." checked={s().notifications.requests} onChange={(v) => patch((x) => (x.notifications.requests = v))} />
                      <Switch label="Spaces" checked={s().notifications.spaces} onChange={(v) => patch((x) => (x.notifications.spaces = v))} />
                      <Switch label="Circles" checked={s().notifications.circles} onChange={(v) => patch((x) => (x.notifications.circles = v))} />
                      <p class="mt-2 text-xs text-muted">Notifications are local Windows toasts driven by sync events. Nothing leaves this PC; there is no push service.</p>
                    </div>
                  </Card>
                </Show>
                <Show when={tab() === "mail"}>
                  <MailTab s={s()} patch={patch} />
                </Show>
                <Show when={tab() === "appearance"}>
                  <Card title={t("settings_appearance")}>
                    <div class="flex flex-col gap-3 p-4">
                      <Field label="Theme">
                        <Select value={s().appearance.theme} onChange={(v) => patch((x) => (x.appearance.theme = v as "dark" | "light" | "system"))} options={[{ value: "dark", label: "Dark" }, { value: "light", label: "Light" }, { value: "system", label: "Follow Windows" }]} />
                      </Field>
                      <Field label="Density">
                        <Select value={s().appearance.density} onChange={(v) => patch((x) => (x.appearance.density = v as "comfortable" | "compact"))} options={[{ value: "comfortable", label: "Comfortable" }, { value: "compact", label: "Compact" }]} />
                      </Field>
                      <Field label="Language">
                        <Select value={s().appearance.language} onChange={(v) => patch((x) => (x.appearance.language = v as "en" | "ka"))} options={LOCALES.map((l) => ({ value: l.id, label: l.label }))} />
                      </Field>
                      <Switch label="Reduce motion" checked={s().appearance.reduced_motion} onChange={(v) => patch((x) => (x.appearance.reduced_motion = v))} />
                      <Switch label="Start with Windows" hint="Starts minimised to the tray at logon." checked={s().start_with_windows} onChange={(v) => patch((x) => (x.start_with_windows = v))} />
                    </div>
                  </Card>
                </Show>
                <Show when={tab() === "updates"}>
                  <UpdatesTab s={s()} patch={patch} />
                </Show>
                <Show when={tab() === "advanced"}>
                  <AdvancedTab s={s()} patch={patch} />
                </Show>
                <Show when={tab() === "about"}>
                  <AboutTab />
                </Show>
              </>
            )}
          </Show>
        </div>
      </div>
    </div>
  );
}

function SecurityTab(props: { s: Settings; patch: (f: (s: Settings) => void) => void }) {
  const [cur, setCur] = createSignal("");
  const [next, setNext] = createSignal("");
  const [next2, setNext2] = createSignal("");
  const [helloPass, setHelloPass] = createSignal("");
  const [busy, setBusy] = createSignal<string | null>(null);
  const [bkPass, setBkPass] = createSignal("");
  const [bkPass2, setBkPass2] = createSignal("");
  const run = async (name: string, f: () => Promise<unknown>, done?: string) => {
    setBusy(name);
    try {
      await f();
      if (done) store.toast(done);
      await store.refreshStatus();
      await store.refreshSettings();
    } catch (e) {
      store.toast(errText(e), "error");
    } finally {
      setBusy(null);
    }
  };
  return (
    <div class="flex flex-col gap-4">
      <Card title="Auto-lock">
        <div class="p-4">
          <Field label="Lock after (minutes of inactivity, 0 = never)" hint="Locking drops every key from memory; the app renders from nothing until you unlock.">
            <Input mono type="number" min="0" class="w-32" value={props.s.security.auto_lock_minutes} onInput={(e) => props.patch((x) => (x.security.auto_lock_minutes = Math.max(0, Number(e.currentTarget.value) || 0)))} />
          </Field>
          <Field label="Clear sensitive clipboard copies after (seconds)" class="mt-3">
            <Input mono type="number" min="5" class="w-32" value={props.s.security.clipboard_clear_secs} onInput={(e) => props.patch((x) => (x.security.clipboard_clear_secs = Math.max(5, Number(e.currentTarget.value) || 30)))} />
          </Field>
        </div>
      </Card>
      <Card title="Windows Hello">
        <div class="flex flex-col gap-3 p-4">
          <p class="text-xs text-muted">{store.status()?.hello_enabled ? "Enabled. Hello wraps the passphrase behind the Windows security chip; it never sees the 24 words." : store.status()?.hello_available ? "Available on this PC. Enrol with your passphrase." : "Not available on this PC."}</p>
          <Show when={!store.status()?.hello_enabled && store.status()?.hello_available}>
            <div class="flex items-center gap-2">
              <Input type="password" class="w-64" placeholder="Confirm your passphrase" value={helloPass()} onInput={(e) => setHelloPass(e.currentTarget.value)} />
              <Button size="sm" loading={busy() === "hello"} disabled={!helloPass()} onClick={() => run("hello", () => ipc.helloEnable(helloPass()).then(() => setHelloPass("")), "Windows Hello enabled")}>
                <Fingerprint size={12} /> Enable
              </Button>
            </div>
          </Show>
          <Show when={store.status()?.hello_enabled}>
            <Button variant="secondary" size="sm" class="w-fit" loading={busy() === "hello-off"} onClick={() => run("hello-off", () => ipc.helloDisable(), "Windows Hello disabled")}>
              Disable Windows Hello
            </Button>
          </Show>
        </div>
      </Card>
      <Card title="Change passphrase">
        <div class="grid grid-cols-3 gap-3 p-4">
          <Field label="Current">
            <Input type="password" value={cur()} onInput={(e) => setCur(e.currentTarget.value)} />
          </Field>
          <Field label="New (10+ characters)">
            <Input type="password" value={next()} onInput={(e) => setNext(e.currentTarget.value)} />
          </Field>
          <Field label="Repeat" error={next2() && next() !== next2() ? "does not match" : undefined}>
            <Input type="password" value={next2()} onInput={(e) => setNext2(e.currentTarget.value)} />
          </Field>
          <div class="col-span-3 flex items-center justify-between">
            <p class="text-xs text-muted">Re-encrypts the vault and locks the app. Windows Hello must be enrolled again.</p>
            <Button size="sm" loading={busy() === "pass"} disabled={!cur() || next().length < 10 || next() !== next2()} onClick={() => run("pass", () => ipc.changePassphrase(cur(), next()))}>
              <KeyRound size={12} /> Change and lock
            </Button>
          </div>
        </div>
      </Card>
      <Card title="Encrypted backup">
        <div class="flex flex-col gap-3 p-4">
          <p class="text-xs text-muted">
            Writes one file with the wallet key, root key and Drive keyring, encrypted with its own passphrase (Argon2id 256 MiB, then XChaCha20-Poly1305). This device's key and encryption sessions are deliberately left out, so a restore becomes a new device. Keep the file anywhere; the host cannot read it.
          </p>
          <div class="grid grid-cols-2 gap-3">
            <Field label="Backup passphrase (12+ characters)" hint="Not this PC's passphrase. Write it down separately.">
              <Input type="password" value={bkPass()} onInput={(e) => setBkPass(e.currentTarget.value)} />
            </Field>
            <Field label="Repeat" error={bkPass2() && bkPass() !== bkPass2() ? "does not match" : undefined}>
              <Input type="password" value={bkPass2()} onInput={(e) => setBkPass2(e.currentTarget.value)} />
            </Field>
          </div>
          <div class="flex gap-2">
            <Button
              size="sm"
              loading={busy() === "backup"}
              disabled={bkPass().length < 12 || bkPass() !== bkPass2()}
              onClick={async () => {
                const p = await pickSavePath(`hashgram-backup-${new Date().toISOString().slice(0, 10)}.hgbkup`, { filters: [{ name: "Hashgram backup", extensions: ["hgbkup"] }] });
                if (!p) return;
                await run("backup", async () => {
                  const m = await ipc.backupExport(p, bkPass());
                  setBkPass(""); setBkPass2("");
                  store.toast(`Backup written for ${m.address.slice(0, 12)}… (${m.partial ? "device-only vault: no account key inside" : "full"})`);
                });
              }}
            >
              <Download size={12} /> Export backup…
            </Button>
            <span class="self-center text-xs text-muted">
              <Upload size={11} class="mr-1 inline" /> To restore: wipe local data (Advanced), then choose "Restore from backup file" on the first screen.
            </span>
          </div>
        </div>
      </Card>
    </div>
  );
}

function MailTab(props: { s: Settings; patch: (f: (s: Settings) => void) => void }) {
  const [ms, setMs] = createSignal<MailSettings | null>(null);
  createResource(
    () => ({ locked: store.locked(), phase: store.phase() }),
    async ({ locked, phase }) => {
      // The SDK sync round owns the facade mutex while it performs bounded
      // network reads. Fetch this local setting as soon as the round reaches
      // Idle instead of leaving the panel apparently stuck behind it.
      const syncOwnsFacade =
        phase === "Connecting" ||
        phase === "Discovering" ||
        (typeof phase === "object" && "Syncing" in phase);
      if (!locked && !syncOwnsFacade && !ms()) {
        setMs(await ipc.mailSettingsGet().catch(() => null));
      }
      return null;
    },
  );
  const patchMs = async (f: (m: MailSettings) => void) => {
    const m = structuredClone(ms());
    if (!m) return;
    f(m);
    setMs(m);
    try {
      await ipc.mailSettingsSet(m);
    } catch (e) {
      store.toast(errText(e), "error");
    }
  };
  return (
    <div class="flex flex-col gap-4">
      <Card title="Privacy">
        <div class="p-4">
          <Show when={ms()} fallback={<p class="text-xs text-muted">{store.locked() ? "Unlock to change mail settings." : t("loading")}</p>}>
            {(m) => (
              <>
                <Switch label="Send read receipts" hint="Only when the sender asked for one. Delivery receipts are always sent when your device decrypts a message." checked={m().send_read_receipts} onChange={(v) => void patchMs((x) => (x.send_read_receipts = v))} />
                <Switch label="Keep sent mail" hint="Your Sent copy, on this device and your other devices." checked={m().keep_sent} onChange={(v) => void patchMs((x) => (x.keep_sent = v))} />
                <Field label="Purge trash and spam after (days, 0 = never)" class="mt-2">
                  <Input mono type="number" min="0" class="w-32" value={m().purge_after_days} onChange={(e) => void patchMs((x) => (x.purge_after_days = Math.max(0, Number(e.currentTarget.value) || 0)))} />
                </Field>
              </>
            )}
          </Show>
        </div>
      </Card>
      <Card title="Reading">
        <div class="p-4">
          <Switch label="Group by conversation" checked={props.s.mail.threaded} onChange={(v) => props.patch((x) => (x.mail.threaded = v))} />
          <Field label="Mark read after (seconds open, 0 = at once)" class="mt-2">
            <Input mono type="number" min="0" class="w-32" value={props.s.mail.mark_read_after_secs} onInput={(e) => props.patch((x) => (x.mail.mark_read_after_secs = Math.max(0, Number(e.currentTarget.value) || 0)))} />
          </Field>
        </div>
      </Card>
    </div>
  );
}

function UpdatesTab(props: { s: Settings; patch: (f: (s: Settings) => void) => void }) {
  return (
    <Card title={t("settings_updates")}>
      <div class="flex flex-col gap-3 p-4">
        <Switch label="Check for updates on start" hint="The only outbound HTTPS request besides endpoints you configured: the signed manifest at the release endpoint." checked={props.s.updates.auto_check} onChange={(v) => props.patch((x) => (x.updates.auto_check = v))} />
        <Field label="Channel">
          <Select value={props.s.updates.channel} onChange={(v) => props.patch((x) => (x.updates.channel = v))} options={[{ value: "stable", label: "Stable" }]} />
        </Field>
        <div class="flex items-center gap-3">
          <Button variant="secondary" size="sm" onClick={() => void updates.checkNow()} loading={updates.busy()}>
            <RefreshCw size={12} /> Check now
          </Button>
          <span class="text-xs text-muted">
            {updates.phase() === "up-to-date" ? "Up to date." : updates.phase() === "available" ? `${updates.available()?.version} is available.` : updates.phase() === "error" ? updates.error() : updates.checkedAt() ? `Checked ${formatMs(updates.checkedAt())}` : ""}
          </span>
          <Show when={updates.phase() === "available"}>
            <Button size="sm" onClick={() => void updates.install()}>
              Install and restart
            </Button>
          </Show>
        </div>
        <p class="text-xs text-muted">
          Manifest and installer are refused unless the minisign signature verifies against the key compiled into this build.
          {store.status()?.updater_configured ? "" : " This build has no updater key; it will not update itself."}
        </p>
      </div>
    </Card>
  );
}

function AdvancedTab(props: { s: Settings; patch: (f: (s: Settings) => void) => void }) {
  const [leases] = createResource(
    () => store.locked(),
    (locked) => (locked ? Promise.resolve([]) : ipc.leasesList().catch(() => [])),
  );
  const [busy, setBusy] = createSignal<string | null>(null);
  const [confirmText, setConfirmText] = createSignal("");
  return (
    <div class="flex flex-col gap-4">
      <Card title="Logging">
        <div class="p-4">
          <Field label="Log level" hint="Logs never contain subjects, bodies, contact addresses or key material. Rotating files under logs/ (7 days).">
            <Select value={props.s.advanced.log_level} onChange={(v) => props.patch((x) => (x.advanced.log_level = v))} options={["error", "warn", "info", "debug"].map((l) => ({ value: l, label: l }))} />
          </Field>
        </div>
      </Card>
      <Card title="Storage leases (protocol v1.1 preview)">
        <div class="p-4 text-xs">
          <p class="mb-2 text-muted">Paid storage leases exist in the client model (signed leases, verify-before-pay). Offering one to a provider needs protocol v1.1; this lists what is recorded locally and can run a verification.</p>
          <Show when={(leases() ?? []).length} fallback={<p class="text-muted">No leases recorded.</p>}>
            <pre class="mono max-h-48 overflow-auto selectable">{JSON.stringify(leases(), null, 1)}</pre>
          </Show>
        </div>
      </Card>
      <Card title="Drive">
        <div class="flex items-center justify-between p-4">
          <p class="text-xs text-muted">Re-encrypt every file under a fresh key and revoke all live shares. Slow: every object is downloaded, re-sealed and uploaded. Use after a device was lost.</p>
          <Button variant="secondary" size="sm" loading={busy() === "rekey"} onClick={async () => { if (await confirm("Rekey every Drive file? This downloads and re-uploads everything and revokes all live shares.")) { setBusy("rekey"); try { const n = await ipc.driveRekeyAll(); store.toast(`${n} file(s) rekeyed`); store.bump("drive"); } catch (e) { store.toast(errText(e), "error"); } finally { setBusy(null); } } }}>
            <KeyRound size={12} /> Rekey all
          </Button>
        </div>
      </Card>
      <Card title="Wipe local data">
        <div class="flex flex-col gap-2 p-4">
          <Notice strong title="Removes the vault, the local store and the UI cache from this PC">You will need your 24 words or a backup file to get back in. Mail and files stored only on this device are lost. The app must be locked first.</Notice>
          <div class="flex items-center gap-2">
            <Input mono class="w-40" placeholder="type DELETE" value={confirmText()} onInput={(e) => setConfirmText(e.currentTarget.value)} />
            <Button variant="danger" size="sm" disabled={confirmText() !== "DELETE"} loading={busy() === "wipe"} onClick={async () => { setBusy("wipe"); try { await ipc.lock(); await ipc.wipeLocalData(confirmText()); await store.refreshStatus(); location.reload(); } catch (e) { store.toast(errText(e), "error"); } finally { setBusy(null); } }}>
              <Trash2 size={12} /> Wipe
            </Button>
          </div>
        </div>
      </Card>
    </div>
  );
}

function AboutTab() {
  const [about] = createResource(() => ipc.aboutInfo());
  const [memBytes] = createResource(() => ipc.perfMemory().catch(() => 0));
  const perfMem = () => memBytes() ?? 0;
  return (
    <Show when={about()}>
      {(a) => (
        <div class="flex flex-col gap-4">
          <Card title={t("app_name")}>
            <div class="grid grid-cols-2 gap-x-6 gap-y-2 p-4 text-[13px]">
              <span class="text-muted">Version</span>
              <span class="mono">
                {a().version} · {a().commit}{" "}
                <Badge brand={a().code_signed} strong={!a().code_signed} title={a().code_signed ? "Authenticode signature present" : "No code-signing certificate yet; SmartScreen may warn on first run"}>
                  {a().code_signed ? <><ShieldCheck size={10} class="mr-1" />{t("signed")}</> : <><ShieldAlert size={10} class="mr-1" />{t("unsigned_preview")}</>}
                </Badge>
              </span>
              <span class="text-muted">Chain</span>
              <span class="mono">{a().chain_id}</span>
              <span class="text-muted">Genesis hash</span>
              <span>
                <Mono text={a().genesis_hash} full copy class="text-xs" />
              </span>
              <span class="text-muted">Vault KDF</span>
              <span>{a().kdf}</span>
              <span class="text-muted">Updater</span>
              <span class="mono truncate text-xs">{a().updater_endpoint || "—"}</span>
              <span class="text-muted">Data</span>
              <span class="flex items-center gap-2">
                <span class="mono truncate text-xs">{a().data_dir}</span>
                <Button variant="ghost" size="sm" onClick={() => void ipc.openDataDir()}>
                  <FolderOpen size={12} />
                </Button>
              </span>
              <span class="text-muted">Memory</span>
              <span class="mono text-xs">{formatBytes(perfMem())}</span>
            </div>
          </Card>
          <Card title="Licenses">
            <ul class="grid grid-cols-2 gap-x-6 p-4 text-xs">
              <For each={a().licenses}>
                {([name, lic]) => (
                  <li class="flex justify-between border-b border-border py-1">
                    <span>{name}</span>
                    <span class="text-muted">{lic}</span>
                  </li>
                )}
              </For>
            </ul>
          </Card>
          <p class="text-[11px] text-muted">{t("tagline")} No analytics, no telemetry, no crash upload. Fonts and scripts are bundled; nothing loads from the Internet.</p>
        </div>
      )}
    </Show>
  );
}
