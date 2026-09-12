// Settings: Network (precedence, HTTPS list, DEVNET), Security, Devices,
// Notifications, Media, Appearance, Updates, Advanced, About.
import { createSignal, createResource, For, Show } from "solid-js";
import { enable as autostartEnable, disable as autostartDisable } from "@tauri-apps/plugin-autostart";
import { save } from "@tauri-apps/plugin-dialog";
import { Card, Button, Field, Input, Notice, Switch, Tabs, Skeleton } from "~/components/ui";
import { Mono } from "~/components/identity";
import { ipc, type Settings as S } from "~/lib/ipc";
import { store } from "~/lib/store";
import { updates } from "~/lib/updates";
import { formatBytes } from "~/lib/format";

const TABS = [
  { id: "network", label: "Network" },
  { id: "security", label: "Security" },
  { id: "notifications", label: "Notifications" },
  { id: "media", label: "Media" },
  { id: "appearance", label: "Appearance" },
  { id: "updates", label: "Updates" },
  { id: "advanced", label: "Advanced" },
  { id: "about", label: "About" },
];

export function Settings() {
  const [tab, setTab] = createSignal("network");
  const [draft, setDraft] = createSignal<S | null>(structuredClone(store.settings()));
  const [busy, setBusy] = createSignal(false);
  const s = () => draft();

  const patch = (f: (d: S) => void) => {
    const d = structuredClone(s());
    if (!d) return;
    f(d);
    setDraft(d);
  };
  const saveSettings = async () => {
    const d = s();
    if (!d) return;
    setBusy(true);
    try {
      await ipc.settingsSet(d);
      await store.refreshSettings();
      await store.refreshStatus();
      document.documentElement.dataset.reducedMotion = d.appearance.reduced_motion ? "true" : "false";
      if (d.start_with_windows) await autostartEnable().catch(() => undefined);
      else await autostartDisable().catch(() => undefined);
      store.toast("Settings saved");
    } catch (e) {
      store.toast(String(e), "error");
    } finally {
      setBusy(false);
    }
  };

  return (
    <div class="page flex flex-col gap-4">
      <div class="flex items-center justify-between">
        <h1 class="page-title">Settings</h1>
        <Button onClick={saveSettings} loading={busy()} disabled={!s()}>
          Save
        </Button>
      </div>
      <Tabs tabs={TABS} value={tab()} onChange={setTab} />
      <Show when={s()} fallback={<Skeleton lines={4} />}>
        {(d) => (
          <div class="fade-in flex flex-col gap-4">
            <Show when={tab() === "network"}>
              <Card title="Network profile">
                <div class="flex flex-col gap-3 p-4">
                  <div class="flex gap-2">
                    <Button variant={d().network.kind === "mainnet" ? "primary" : "secondary"} onClick={() => patch((x) => (x.network.kind = "mainnet"))}>
                      Mainnet
                    </Button>
                    <Button variant={d().network.kind === "devnet" ? "primary" : "secondary"} onClick={() => patch((x) => (x.network.kind = "devnet"))}>
                      DEVNET
                    </Button>
                  </div>
                  <Show when={d().network.kind === "devnet"}>
                    <Notice strong title="DEVNET is visually unmistakable">A persistent banner is shown while this profile is active. Nothing on a devnet has value.</Notice>
                    <Field label="Devnet genesis hash (64 hex)">
                      <Input mono value={d().network.devnet_genesis_hash} onInput={(e) => patch((x) => (x.network.devnet_genesis_hash = e.currentTarget.value))} />
                    </Field>
                    <Field label="Devnet bootstrap multiaddrs (one per line)">
                      <textarea class="textarea mono" rows={3} value={d().network.devnet_bootstrap.join("\n")} onInput={(e) => patch((x) => (x.network.devnet_bootstrap = e.currentTarget.value.split("\n").map((l) => l.trim()).filter(Boolean)))} />
                    </Field>
                  </Show>
                  <Show when={d().network.kind === "mainnet"}>
                    <p class="text-xs text-muted">
                      Mainnet is pinned to genesis <span class="mono">e322bc23…d5e4d</span>, compiled into the app. Bootstrap peers come from the built-in list and the peerstore; nothing to type.
                    </p>
                  </Show>
                </div>
              </Card>
              <Card title="Chain sources (precedence)">
                <div class="flex flex-col gap-3 p-4">
                  <Field label="1. Node on this PC (REST gateway)" hint="Used first when it answers on the right chain.">
                    <Input mono value={d().network.local_node_api} onInput={(e) => patch((x) => (x.network.local_node_api = e.currentTarget.value))} />
                  </Field>
                  <p class="text-sm">2. P2P chain relay across ≥ 2 connected nodes, cross-checked (always on).</p>
                  <Field label="3. HTTPS REST endpoints (one per line)" hint="Optional fallbacks you trust. Never the only way in; each is checked against the chain id.">
                    <textarea class="textarea mono" rows={3} value={d().network.https_endpoints.join("\n")} onInput={(e) => patch((x) => (x.network.https_endpoints = e.currentTarget.value.split("\n").map((l) => l.trim()).filter(Boolean)))} placeholder="https://rest.example.org" />
                  </Field>
                </div>
              </Card>
            </Show>

            <Show when={tab() === "security"}>
              <SecurityTab d={d()} patch={patch} />
            </Show>

            <Show when={tab() === "notifications"}>
              <Card>
                <div class="p-4">
                  <Switch label="Messages" hint="Native Windows toast for new messages" checked={d().notifications.messages} onChange={(v) => patch((x) => (x.notifications.messages = v))} />
                  <Switch label="Calls" hint="Toast for incoming calls" checked={d().notifications.calls} onChange={(v) => patch((x) => (x.notifications.calls = v))} />
                </div>
              </Card>
            </Show>

            <Show when={tab() === "media"}>
              <Card>
                <div class="flex flex-col gap-3 p-4">
                  <Switch label="Autoplay reels and videos" checked={d().media.autoplay} onChange={(v) => patch((x) => (x.media.autoplay = v))} />
                  <Field label="Media cache ceiling (MiB)">
                    <Input mono type="number" min="128" value={d().media.cache_mb} onInput={(e) => patch((x) => (x.media.cache_mb = Number(e.currentTarget.value) || 512))} />
                  </Field>
                  <p class="text-xs text-muted">
                    Storage location: <span class="mono">{store.status()?.data_dir}\media-cache</span>
                  </p>
                </div>
              </Card>
            </Show>

            <Show when={tab() === "appearance"}>
              <Card>
                <div class="p-4">
                  <p class="mb-2 text-sm text-muted">Monochrome brand chrome: black, white and greys. Your content — photos, videos, avatars — stays in colour. Dark only.</p>
                  <Switch label="Reduce motion" hint="Also follows the Windows setting automatically" checked={d().appearance.reduced_motion} onChange={(v) => patch((x) => (x.appearance.reduced_motion = v))} />
                  <Switch label="Compact density" checked={d().appearance.compact} onChange={(v) => patch((x) => (x.appearance.compact = v))} />
                </div>
              </Card>
            </Show>

            <Show when={tab() === "updates"}>
              <UpdatesTab d={d()} patch={patch} />
            </Show>

            <Show when={tab() === "advanced"}>
              <Card>
                <div class="flex flex-col gap-3 p-4">
                  <Field label="Log level" hint="Takes effect on restart.">
                    <select class="input" value={d().advanced.log_level} onChange={(e) => patch((x) => (x.advanced.log_level = e.currentTarget.value))}>
                      <For each={["error", "warn", "info", "debug", "trace"]}>{(l) => <option value={l}>{l}</option>}</For>
                    </select>
                  </Field>
                  <Switch label="Start with Windows" hint="Starts minimised to the tray" checked={d().start_with_windows} onChange={(v) => patch((x) => (x.start_with_windows = v))} />
                  <div class="flex gap-2">
                    <Button variant="secondary" onClick={() => void ipc.openDataDir()}>
                      Open data folder
                    </Button>
                    <Button
                      variant="secondary"
                      onClick={async () => {
                        try {
                          const text = await ipc.diagnosticsExport();
                          const path = await save({ defaultPath: "hashgram-logs.txt" });
                          if (path) {
                            await ipc.saveTextFile(path, text);
                            store.toast("Exported (secrets are never logged)");
                          }
                        } catch (e) {
                          store.toast(String(e), "error");
                        }
                      }}
                    >
                      Export logs
                    </Button>
                  </div>
                </div>
              </Card>
            </Show>

            <Show when={tab() === "about"}>
              <Card>
                <dl class="grid grid-cols-[auto_1fr] gap-x-6 gap-y-1 p-4 text-sm">
                  <dt class="text-muted">Version</dt>
                  <dd class="mono">{store.status()?.version}</dd>
                  <dt class="text-muted">Commit</dt>
                  <dd class="mono">{store.status()?.commit}</dd>
                  <dt class="text-muted">Chain</dt>
                  <dd class="mono">{store.status()?.chain_id}</dd>
                  <dt class="text-muted">Data</dt>
                  <dd class="mono">{store.status()?.data_dir}</dd>
                  <dt class="text-muted">Licence</dt>
                  <dd>Apache-2.0. Built with Tauri, SolidJS, libp2p, OpenMLS, rusqlite, Lucide icons.</dd>
                  <dt class="text-muted">Telemetry</dt>
                  <dd>None. No analytics, no crash uploads. The only outbound HTTPS is the signed update check and endpoints you configured.</dd>
                </dl>
              </Card>
            </Show>
          </div>
        )}
      </Show>
    </div>
  );
}

function SecurityTab(props: { d: S; patch: (f: (d: S) => void) => void }) {
  const [cur, setCur] = createSignal("");
  const [n1, setN1] = createSignal("");
  const [n2, setN2] = createSignal("");
  const [helloPass, setHelloPass] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const change = async () => {
    setBusy(true);
    try {
      await ipc.changePassphrase(cur(), n1());
      store.toast("Passphrase changed. Unlock again with the new one.");
      await store.refreshStatus();
    } catch (e) {
      store.toast(String(e), "error");
    } finally {
      setBusy(false);
    }
  };
  const helloOn = async () => {
    setBusy(true);
    try {
      await ipc.helloEnable(helloPass());
      setHelloPass("");
      await store.refreshStatus();
      await store.refreshSettings();
      store.toast("Windows Hello enabled");
    } catch (e) {
      store.toast(String(e), "error");
    } finally {
      setBusy(false);
    }
  };
  const helloOff = async () => {
    setBusy(true);
    try {
      await ipc.helloDisable();
      await store.refreshStatus();
      await store.refreshSettings();
      store.toast("Windows Hello disabled");
    } catch (e) {
      store.toast(String(e), "error");
    } finally {
      setBusy(false);
    }
  };
  return (
    <>
      <Card title="Auto-lock and clipboard">
        <div class="grid grid-cols-2 gap-3 p-4">
          <Field label="Lock after inactivity (minutes, 0 = never)">
            <Input mono type="number" min="0" value={props.d.security.auto_lock_minutes} onInput={(e) => props.patch((x) => (x.security.auto_lock_minutes = Number(e.currentTarget.value) || 0))} />
          </Field>
          <Field label="Clear sensitive clipboard copies after (seconds)">
            <Input mono type="number" min="5" value={props.d.security.clipboard_clear_secs} onInput={(e) => props.patch((x) => (x.security.clipboard_clear_secs = Number(e.currentTarget.value) || 30))} />
          </Field>
        </div>
      </Card>
      <Card title="Windows Hello">
        <div class="flex flex-col gap-3 p-4">
          <Show when={store.status()?.hello_available} fallback={<Notice>Windows Hello is not set up on this PC (Settings → Accounts → Sign-in options in Windows).</Notice>}>
            <Show
              when={store.status()?.hello_enabled}
              fallback={
                <>
                  <p class="text-sm text-muted">Wraps your passphrase behind a Hello prompt using DPAPI. Hello never sees your 24 words.</p>
                  <Field label="Confirm passphrase to enrol">
                    <Input type="password" value={helloPass()} onInput={(e) => setHelloPass(e.currentTarget.value)} />
                  </Field>
                  <div>
                    <Button onClick={helloOn} loading={busy()} disabled={!helloPass()}>
                      Enable Windows Hello
                    </Button>
                  </div>
                </>
              }
            >
              <p class="text-sm">Enabled.</p>
              <div>
                <Button variant="secondary" onClick={helloOff} loading={busy()}>
                  Disable Windows Hello
                </Button>
              </div>
            </Show>
          </Show>
        </div>
      </Card>
      <Card title="Change passphrase">
        <div class="grid grid-cols-3 gap-3 p-4">
          <Field label="Current">
            <Input type="password" value={cur()} onInput={(e) => setCur(e.currentTarget.value)} />
          </Field>
          <Field label="New (≥ 8 characters)">
            <Input type="password" value={n1()} onInput={(e) => setN1(e.currentTarget.value)} />
          </Field>
          <Field label="Repeat" error={n2() && n1() !== n2() ? "Does not match" : undefined}>
            <Input type="password" value={n2()} onInput={(e) => setN2(e.currentTarget.value)} />
          </Field>
          <div class="col-span-3 flex items-center justify-between">
            <p class="text-xs text-muted">Re-encrypts the vault on this PC. Windows Hello must be re-enrolled afterwards. The 24 words are unaffected.</p>
            <Button variant="secondary" onClick={change} loading={busy()} disabled={!cur() || n1().length < 8 || n1() !== n2()}>
              Change
            </Button>
          </div>
        </div>
      </Card>
      <Notice title="Forgot passphrase">There is no reset. Lock the app and use “Forgot passphrase?” on the lock screen to remove this PC's data and restore from your 24 words.</Notice>
    </>
  );
}

function UpdatesTab(props: { d: S; patch: (f: (d: S) => void) => void }) {
  const status = () => store.status();
  const configured = () => status()?.updater_configured;
  const phase = updates.phase;
  const u = updates.available;
  const statusLine = () => {
    switch (phase()) {
      case "checking":
        return "Checking the signed manifest…";
      case "up-to-date":
        return `Up to date (v${status()?.version ?? ""}).`;
      case "available":
        return `Hashgram ${u()?.version} is available${u()?.date ? ` (${u()?.date})` : ""}.`;
      case "downloading": {
        const p = updates.progress();
        return p.total ? `Downloading… ${formatBytes(p.done)} / ${formatBytes(p.total)}` : `Downloading… ${formatBytes(p.done)}`;
      }
      case "installing":
        return "Signature verified. Installing and restarting…";
      case "error":
        return `Update failed: ${updates.error()}`;
      default:
        return "";
    }
  };
  const pctDone = () => {
    const p = updates.progress();
    return p.total ? Math.min(100, Math.round((p.done / p.total) * 100)) : 0;
  };
  return (
    <Card title="Updates">
      <div class="flex flex-col gap-3 p-4">
        <Show when={!configured()}>
          <Notice strong title="Updater not configured in this build">
            The signing public key compiled into this build is a placeholder. The owner generates a minisign keypair offline, puts the public key in <span class="mono">tauri.conf.json</span> and signs releases with the private key, which never touches a server. Until then “Check now” refuses unsigned manifests by design.
          </Notice>
        </Show>
        <Switch label="Check for updates on start" hint="The only outbound HTTPS this app makes on its own. The manifest and the installer must carry a valid signature; anything else is refused." checked={props.d.updates.auto_check} onChange={(v) => props.patch((x) => (x.updates.auto_check = v))} />
        <div class="flex items-center gap-3">
          <Button variant="secondary" onClick={() => void updates.checkNow()} loading={phase() === "checking"} disabled={updates.busy() || !configured()}>
            Check now
          </Button>
          <Show when={phase() === "available"}>
            <Button onClick={() => void updates.install()} disabled={updates.busy()}>
              Install v{u()?.version} and restart
            </Button>
          </Show>
          <span class="whitespace-pre-wrap text-xs text-muted" role="status">
            {statusLine()}
          </span>
        </div>
        <Show when={phase() === "downloading" && updates.progress().total}>
          <div class="h-1 w-full overflow-hidden rounded bg-surface-2" aria-hidden="true">
            <div class="h-full bg-fg transition-[width]" style={{ width: `${pctDone()}%` }} />
          </div>
        </Show>
        <Show when={phase() === "available" && u()?.body}>
          <div class="rounded-md border border-border bg-surface-2 p-3 text-xs whitespace-pre-wrap">{u()?.body}</div>
        </Show>
        <div class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-xs text-muted">
          <span>Installed</span>
          <span class="mono">
            v{status()?.version} · {status()?.commit}
          </span>
          <span>Source</span>
          <span class="mono break-all">{status()?.updater_endpoint || "—"}</span>
          <span>Signing key id</span>
          <Mono text={configured() ? status()?.updater_key_id || "configured" : "REPLACE_WITH_MINISIGN_PUBLIC_KEY (placeholder)"} full />
          <span>Install mode</span>
          <span>Passive installer for the current user, no administrator prompt; your vault and settings stay in place.</span>
        </div>
      </div>
    </Card>
  );
}
