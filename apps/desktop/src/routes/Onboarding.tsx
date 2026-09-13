// First run. Creating an identity = generating 24 words. Restoring = the
// same words, or an encrypted backup file. Nothing else exists: no
// sign-up, no e-mail, no server. The last step says honestly what still
// gates messaging: an on-chain registration that needs a little HASH.
import { createSignal, Show, For, createResource, createMemo } from "solid-js";
import { KeyRound, Download, FileKey, Fingerprint, AtSign, ShieldCheck } from "lucide-solid";
import { Button, Field, Input, Notice, Textarea, ProgressBar } from "~/components/ui";
import { Mono } from "~/components/identity";
import { MnemonicDisplay } from "~/components/MnemonicDisplay";
import { Logo } from "~/components/Shell";
import { ipc, errText, type AccountInfo } from "~/lib/ipc";
import { store } from "~/lib/store";
import { t } from "~/lib/i18n";
import { pickFile } from "~/lib/dialogs";
import { formatHash } from "~/lib/format";

type Step =
  | { s: "welcome" }
  | { s: "words"; words: string[] }
  | { s: "check"; words: string[]; positions: number[] }
  | { s: "passphrase"; mode: "create" | "restore" | "backup"; mnemonic?: string; backupPath?: string; backupPass?: string }
  | { s: "hello"; info: AccountInfo }
  | { s: "identity"; info: AccountInfo }
  | { s: "restore" }
  | { s: "restore-confirm"; mnemonic: string; address: string }
  | { s: "backup" };

function pickThree(): number[] {
  const set = new Set<number>();
  while (set.size < 3) set.add(Math.floor(Math.random() * 24));
  return [...set].sort((a, b) => a - b);
}

/** 0–4 from length and character classes. Never sent anywhere. */
export function passphraseStrength(p: string): number {
  let score = 0;
  if (p.length >= 10) score++;
  if (p.length >= 14) score++;
  if (/[a-z]/.test(p) && /[A-Z]/.test(p)) score++;
  if (/\d/.test(p) && /[^A-Za-z0-9]/.test(p)) score++;
  if (p.length >= 20) score = Math.min(4, score + 1);
  return Math.min(4, score);
}

function StrengthMeter(props: { value: string }) {
  const s = () => passphraseStrength(props.value);
  const label = () => ["too short", "weak", "fair", "good", "strong"][s()];
  return (
    <div class="mt-1 flex items-center gap-2">
      <ProgressBar value={props.value.length < 10 ? 0 : s()} max={4} class="w-40" label="strength" />
      <span class="text-xs text-muted">{props.value ? label() : ""}</span>
    </div>
  );
}

export function Onboarding(props: { onDone: () => void }) {
  const [step, setStep] = createSignal<Step>({ s: "welcome" });
  const [error, setError] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal(false);
  const fail = (e: unknown) => setError(errText(e));

  const startCreate = async () => {
    setError(null);
    setBusy(true);
    try {
      const words = await ipc.onboardingGenerate();
      setStep({ s: "words", words });
    } catch (e) {
      fail(e);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div class="flex h-full items-center justify-center overflow-auto p-8">
      <div class="w-full max-w-xl fade-in">
        <Show when={error()}>
          <div class="mb-4">
            <Notice strong title={t("error_title")}>
              {error()}
            </Notice>
          </div>
        </Show>

        <Show when={step().s === "welcome"}>
          <div class="flex flex-col gap-6">
            <div class="flex items-center gap-3">
              <Logo size={34} />
              <div>
                <h1 class="text-2xl font-semibold tracking-tight">{t("app_name")}</h1>
                <p class="text-sm text-muted">{t("tagline")}</p>
              </div>
            </div>
            <div class="grid grid-cols-1 gap-3">
              <button type="button" class="card flex items-start gap-3 p-4 text-left transition-colors hover:border-brand" onClick={startCreate} disabled={busy()}>
                <KeyRound size={18} class="mt-0.5 text-brand" />
                <span>
                  <span class="block font-medium">{t("onboarding_create")}</span>
                  <span class="block text-xs text-muted">{t("onboarding_create_hint")}</span>
                </span>
              </button>
              <button type="button" class="card flex items-start gap-3 p-4 text-left transition-colors hover:border-brand" onClick={() => setStep({ s: "restore" })}>
                <Download size={18} class="mt-0.5 text-muted" />
                <span>
                  <span class="block font-medium">{t("onboarding_restore")}</span>
                  <span class="block text-xs text-muted">{t("onboarding_restore_hint")}</span>
                </span>
              </button>
              <button type="button" class="card flex items-start gap-3 p-4 text-left transition-colors hover:border-brand" onClick={() => setStep({ s: "backup" })}>
                <FileKey size={18} class="mt-0.5 text-muted" />
                <span>
                  <span class="block font-medium">{t("onboarding_backup")}</span>
                  <span class="block text-xs text-muted">{t("onboarding_backup_hint")}</span>
                </span>
              </button>
            </div>
            <Notice>An identity is a key. There is no sign-up, no e-mail and no password reset by anyone. If you lose the 24 words and forget your passphrase, nobody can help — that is the point.</Notice>
          </div>
        </Show>

        <Show when={step().s === "words"}>
          {(_) => {
            const st = step() as Extract<Step, { s: "words" }>;
            return (
              <div class="flex flex-col gap-4">
                <h1 class="text-lg font-semibold">Your 24 words</h1>
                <p class="text-sm text-muted">
                  Write them down on paper, in order. They are shown once. Never type them into a website, never store them in the cloud. The app will not copy them to the clipboard and blurs them when this window loses focus; a screenshot is still your responsibility.
                </p>
                <MnemonicDisplay words={st.words} />
                <div class="flex justify-between">
                  <Button variant="ghost" onClick={() => setStep({ s: "welcome" })}>
                    {t("back")}
                  </Button>
                  <Button onClick={() => setStep({ s: "check", words: st.words, positions: pickThree() })}>I wrote them down</Button>
                </div>
              </div>
            );
          }}
        </Show>

        <Show when={step().s === "check"}>
          {(_) => {
            const st = step() as Extract<Step, { s: "check" }>;
            const [typed, setTyped] = createSignal<string[]>(["", "", ""]);
            const [wrong, setWrong] = createSignal(false);
            const verify = async () => {
              setWrong(false);
              const ok = await ipc.onboardingCheckWords(st.positions, typed()).catch(() => false);
              if (ok) setStep({ s: "passphrase", mode: "create" });
              else setWrong(true);
            };
            return (
              <div class="flex flex-col gap-4">
                <h1 class="text-lg font-semibold">Check three words</h1>
                <p class="text-sm text-muted">Type the words at these positions from your paper copy.</p>
                <div class="grid grid-cols-3 gap-3">
                  <For each={st.positions}>
                    {(pos, i) => (
                      <Field label={`Word #${pos + 1}`}>
                        <Input
                          mono
                          autocomplete="off"
                          value={typed()[i()] ?? ""}
                          onInput={(e) => {
                            const arr = [...typed()];
                            arr[i()] = e.currentTarget.value;
                            setTyped(arr);
                          }}
                          onKeyDown={(e) => e.key === "Enter" && void verify()}
                        />
                      </Field>
                    )}
                  </For>
                </div>
                <Show when={wrong()}>
                  <Notice strong>One or more words do not match. Check your paper copy.</Notice>
                </Show>
                <div class="flex justify-between">
                  <Button variant="ghost" onClick={() => setStep({ s: "words", words: st.words })}>
                    Show the words again
                  </Button>
                  <Button onClick={verify} disabled={typed().some((w) => !w.trim())}>
                    {t("continue")}
                  </Button>
                </div>
              </div>
            );
          }}
        </Show>

        <Show when={step().s === "restore"}>
          {(_) => {
            const [text, setText] = createSignal("");
            const count = () => text().trim().split(/\s+/).filter(Boolean).length;
            const preview = async () => {
              setError(null);
              setBusy(true);
              try {
                const r = await ipc.onboardingRestorePreview(text());
                setStep({ s: "restore-confirm", mnemonic: text(), address: r.address });
              } catch (e) {
                fail(e);
              } finally {
                setBusy(false);
              }
            };
            return (
              <div class="flex flex-col gap-4">
                <h1 class="text-lg font-semibold">{t("onboarding_restore")}</h1>
                <Field label="Your 24 words, separated by spaces" hint={`${count()} / 24 words`}>
                  <Textarea mono rows={4} value={text()} onInput={(e) => setText(e.currentTarget.value)} autocomplete="off" data-secret="true" placeholder="word word word …" />
                </Field>
                <div class="flex justify-between">
                  <Button variant="ghost" onClick={() => setStep({ s: "welcome" })}>
                    {t("back")}
                  </Button>
                  <Button onClick={preview} disabled={count() !== 24} loading={busy()}>
                    Show my address
                  </Button>
                </div>
              </div>
            );
          }}
        </Show>

        <Show when={step().s === "restore-confirm"}>
          {(_) => {
            const st = step() as Extract<Step, { s: "restore-confirm" }>;
            return (
              <div class="flex flex-col gap-4">
                <h1 class="text-lg font-semibold">Is this your address?</h1>
                <div class="card p-4">
                  <Mono text={st.address} full class="text-sm" />
                </div>
                <p class="text-sm text-muted">If this is not the address you expected, a word is wrong or out of order. Nothing has been saved yet.</p>
                <div class="flex justify-between">
                  <Button variant="ghost" onClick={() => setStep({ s: "restore" })}>
                    Fix the words
                  </Button>
                  <Button onClick={() => setStep({ s: "passphrase", mode: "restore", mnemonic: st.mnemonic })}>Yes, continue</Button>
                </div>
              </div>
            );
          }}
        </Show>

        <Show when={step().s === "backup"}>
          {(_) => {
            const [path, setPath] = createSignal("");
            const [pass, setPass] = createSignal("");
            const [info] = createResource(
              () => path(),
              (p) => (p ? ipc.backupInspect(p) : Promise.resolve(null)),
            );
            const choose = async () => {
              const f = await pickFile({ title: "Choose a Hashgram backup", filters: [{ name: "Hashgram backup", extensions: ["hgbkup", "bin", "*"] }] });
              if (f[0]) setPath(f[0]);
            };
            return (
              <div class="flex flex-col gap-4">
                <h1 class="text-lg font-semibold">{t("onboarding_backup")}</h1>
                <div class="flex items-center gap-2">
                  <Button variant="secondary" onClick={choose}>
                    Choose file…
                  </Button>
                  <span class="mono truncate text-xs text-muted">{path() || "no file chosen"}</span>
                </div>
                <Show when={info.error}>
                  <Notice strong>{errText(info.error)}</Notice>
                </Show>
                <Show when={info()}>
                  {(i) => (
                    <p class="text-xs text-muted">
                      Hashgram backup, {(i().bytes / 1024).toFixed(1)} KiB, Argon2id {Math.round(i().m_cost_kib / 1024)} MiB × {i().t_cost}. Decrypting takes a few seconds.
                    </p>
                  )}
                </Show>
                <Field label="Backup passphrase" hint="The passphrase chosen when the backup was exported (not this PC's passphrase).">
                  <Input type="password" value={pass()} onInput={(e) => setPass(e.currentTarget.value)} autocomplete="off" />
                </Field>
                <div class="flex justify-between">
                  <Button variant="ghost" onClick={() => setStep({ s: "welcome" })}>
                    {t("back")}
                  </Button>
                  <Button disabled={!info() || pass().length < 12} onClick={() => setStep({ s: "passphrase", mode: "backup", backupPath: path(), backupPass: pass() })}>
                    {t("continue")}
                  </Button>
                </div>
              </div>
            );
          }}
        </Show>

        <Show when={step().s === "passphrase"}>
          {(_) => {
            const st = step() as Extract<Step, { s: "passphrase" }>;
            const [p1, setP1] = createSignal("");
            const [p2, setP2] = createSignal("");
            const [label, setLabel] = createSignal("");
            const ok = () => p1().length >= 10 && p1() === p2();
            const go = async () => {
              setError(null);
              setBusy(true);
              try {
                const info =
                  st.mode === "create"
                    ? await ipc.onboardingCreate(p1(), label())
                    : st.mode === "restore"
                      ? await ipc.onboardingRestore(st.mnemonic ?? "", p1(), label())
                      : await ipc.restoreFromBackup(st.backupPath ?? "", st.backupPass ?? "", p1(), label());
                await store.refreshStatus();
                await store.refreshSettings();
                setStep({ s: "hello", info });
              } catch (e) {
                fail(e);
              } finally {
                setBusy(false);
              }
            };
            return (
              <div class="flex flex-col gap-4">
                <h1 class="text-lg font-semibold">Set a passphrase for this PC</h1>
                <p class="text-sm text-muted">It encrypts your keys on this computer (Argon2id, 64 MiB). Forgot it? Restore from the 24 words or a backup — there is no other reset.</p>
                <Field label={t("passphrase")} hint="At least 10 characters. A sentence is easier to remember and stronger than a word.">
                  <Input type="password" value={p1()} onInput={(e) => setP1(e.currentTarget.value)} autocomplete="new-password" />
                  <StrengthMeter value={p1()} />
                </Field>
                <Field label="Repeat" error={p2() && p1() !== p2() ? "Does not match" : undefined}>
                  <Input type="password" value={p2()} onInput={(e) => setP2(e.currentTarget.value)} autocomplete="new-password" onKeyDown={(e) => e.key === "Enter" && ok() && void go()} />
                </Field>
                <Field label="Name this device" hint="Shown to your other devices and registered on chain as this device's label.">
                  <Input value={label()} onInput={(e) => setLabel(e.currentTarget.value)} placeholder="Home PC" />
                </Field>
                <div class="flex justify-end">
                  <Button onClick={go} disabled={!ok()} loading={busy()}>
                    {st.mode === "create" ? "Create vault" : "Restore"}
                  </Button>
                </div>
              </div>
            );
          }}
        </Show>

        <Show when={step().s === "hello"}>
          {(_) => {
            const st = step() as Extract<Step, { s: "hello" }>;
            const [pass, setPass] = createSignal("");
            const enable = async () => {
              setError(null);
              setBusy(true);
              try {
                await ipc.helloEnable(pass());
                store.toast("Windows Hello enabled");
                setStep({ s: "identity", info: st.info });
              } catch (e) {
                fail(e);
              } finally {
                setBusy(false);
              }
            };
            return (
              <div class="flex flex-col gap-4">
                <div class="flex items-center gap-2">
                  <Fingerprint size={18} />
                  <h1 class="text-lg font-semibold">Windows Hello (optional)</h1>
                </div>
                <p class="text-sm text-muted">Unlock day to day with your face, fingerprint or PIN. Hello wraps your passphrase behind the Windows security chip; it never sees the 24 words.</p>
                <Show when={store.status()?.hello_available} fallback={<Notice>Windows Hello is not set up on this PC. You can enable it later in Settings → Security.</Notice>}>
                  <Field label="Confirm your passphrase to enrol">
                    <Input type="password" value={pass()} onInput={(e) => setPass(e.currentTarget.value)} autocomplete="current-password" />
                  </Field>
                </Show>
                <div class="flex justify-between">
                  <Button variant="ghost" onClick={() => setStep({ s: "identity", info: st.info })}>
                    Skip
                  </Button>
                  <Button onClick={enable} disabled={!store.status()?.hello_available || !pass()} loading={busy()}>
                    Enable Windows Hello
                  </Button>
                </div>
              </div>
            );
          }}
        </Show>

        <Show when={step().s === "identity"}>
          {(_) => {
            const st = step() as Extract<Step, { s: "identity" }>;
            return <IdentityStep info={st.info} onDone={props.onDone} />;
          }}
        </Show>
      </div>
    </div>
  );
}

/** The honest last step: address, balance, register when funded, username. */
export function IdentityStep(props: { info: AccountInfo; onDone: () => void; embedded?: boolean }) {
  const [tick, setTick] = createSignal(0);
  const [status, { refetch }] = createResource(
    () => tick(),
    () => ipc.identityStatus().catch(() => null),
  );
  const [qr] = createResource(() => props.info.address, (a) => ipc.qrSvg(a).catch(() => ""));
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const balance = createMemo(() => BigInt(status()?.balance_uhash ?? "0"));
  // The relayed chain transport cannot simulate, so identity registration
  // uses the SDK's conservative gas estimate. 0.01 HASH leaves ample room
  // for that one-time fee instead of enabling a button that may still fail.
  const funded = () => balance() >= 10_000n;
  const registered = () => !!status()?.this_device_registered;
  const register = async () => {
    setBusy(true);
    setError(null);
    try {
      const r = await ipc.identityRegister(store.settings()?.device_label ?? "");
      store.toast(`${r.summary} — submitted`);
      setTimeout(() => setTick((n) => n + 1), 6000);
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div class="flex flex-col gap-4">
      <Show when={!props.embedded}>
        <div class="flex items-center gap-2">
          <ShieldCheck size={18} />
          <h1 class="text-lg font-semibold">Your identity</h1>
        </div>
      </Show>
      <div class="card flex items-center gap-4 p-4">
        <Show when={qr()}>
          <div class="h-24 w-24 shrink-0 rounded bg-fg p-1" innerHTML={qr()} aria-label="address QR" />
        </Show>
        <div class="min-w-0">
          <p class="mb-1 text-xs text-muted">Your address</p>
          <Mono text={props.info.address} full copy class="break-all text-xs" />
          <p class="mt-2 text-xs text-muted">
            Balance: <span class="tnum text-fg">{status() ? `${formatHash(status()!.balance_uhash ?? "0")} HASH` : "…"}</span>
            {status() && !status()!.online ? " (chain not reachable yet)" : ""}
          </p>
        </div>
      </div>
      <p class="text-sm text-muted">{t("identity_gate")}</p>
      <Show when={error()}>
        <Notice strong>{error()}</Notice>
      </Show>
      <Show
        when={registered()}
        fallback={
          <Show
            when={funded()}
            fallback={
              <Notice title={t("identity_needs_hash")}>
                Send at least 0.01 HASH to the address above so this PC can pay the one-time identity registration fee. The app checks on every sync. Until then, Drive, drafts and settings work locally; posting, usernames, and sending or receiving mail do not.
              </Notice>
            }
          >
            <Notice title="Ready to register">
              The account holds HASH. Registering puts this device's public key on chain in one transaction; the fee is a fraction of a HASH.
            </Notice>
          </Show>
        }
      >
        <Notice title="Registered">This device's key is on chain. You can send and receive mail.</Notice>
      </Show>
      <div class="flex items-center justify-between gap-2">
        <Button variant="ghost" onClick={() => void refetch()}>
          Check again
        </Button>
        <div class="flex gap-2">
          <Show when={!registered()}>
            <Button onClick={register} disabled={!funded() || busy()} loading={busy()}>
              {t("identity_register")}
            </Button>
          </Show>
          <Show when={registered() && !status()?.username}>
            <Button variant="secondary" onClick={props.onDone}>
              <AtSign size={14} /> Choose a username later
            </Button>
          </Show>
          <Button variant={registered() ? "primary" : "secondary"} onClick={props.onDone}>
            {registered() ? "Open Hashgram" : "Use local features only"}
          </Button>
        </div>
      </div>
    </div>
  );
}
