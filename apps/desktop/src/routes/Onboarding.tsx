// First run. Creating an account = generating 24 words. Logging in =
// restoring 24 words. Nothing else exists: no sign-up, no email, no server.
import { createSignal, Show, For, createResource, onMount } from "solid-js";
import { KeyRound, Download, ShieldCheck, Fingerprint, AtSign, Network as NetIcon } from "lucide-solid";
import { Button, Field, Input, Notice, Textarea, Skeleton } from "~/components/ui";
import { Mono } from "~/components/identity";
import { ipc, type AccountInfo } from "~/lib/ipc";
import { store } from "~/lib/store";

type Step =
  | { s: "welcome" }
  | { s: "words"; words: string[] }
  | { s: "check"; words: string[]; positions: number[] }
  | { s: "passphrase"; mode: "create" | "restore"; mnemonic?: string }
  | { s: "hello"; info: AccountInfo }
  | { s: "username"; info: AccountInfo }
  | { s: "connecting"; info: AccountInfo }
  | { s: "restore" }
  | { s: "restore-confirm"; mnemonic: string; address: string };

function pickThree(): number[] {
  const set = new Set<number>();
  while (set.size < 3) set.add(Math.floor(Math.random() * 24));
  return [...set].sort((a, b) => a - b);
}

export function Onboarding(props: { onDone: () => void }) {
  const [step, setStep] = createSignal<Step>({ s: "welcome" });
  const [error, setError] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal(false);

  const fail = (e: unknown) => setError(String(e));

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
            <Notice strong title="Something went wrong">
              {error()}
            </Notice>
          </div>
        </Show>

        <Show when={step().s === "welcome"}>
          <div class="flex flex-col gap-6">
            <div class="flex items-center gap-3">
              <svg viewBox="0 0 64 64" width="36" height="36" aria-hidden="true">
                <rect x="21" y="12" width="6" height="40" fill="#ffffff" />
                <rect x="37" y="12" width="6" height="40" fill="#ffffff" />
                <rect x="12" y="21" width="40" height="6" fill="#ffffff" />
                <rect x="12" y="37" width="40" height="6" fill="#ffffff" />
              </svg>
              <div>
                <h1 class="text-2xl font-semibold tracking-tight">Hashgram</h1>
                <p class="text-sm text-muted">Wallet, messenger, social network, calls and identity — on the peer-to-peer network, never one server.</p>
              </div>
            </div>
            <div class="grid grid-cols-2 gap-3">
              <button type="button" class="card flex flex-col gap-2 p-5 text-left transition-colors hover:border-accent" onClick={startCreate} disabled={busy()}>
                <KeyRound size={20} />
                <span class="font-medium">Create wallet</span>
                <span class="text-xs text-muted">Generates 24 words. They are your account; write them down.</span>
              </button>
              <button type="button" class="card flex flex-col gap-2 p-5 text-left transition-colors hover:border-accent" onClick={() => setStep({ s: "restore" })}>
                <Download size={20} />
                <span class="font-medium">Restore</span>
                <span class="text-xs text-muted">Type your 24 words. The same words give the same account on any PC.</span>
              </button>
            </div>
            <Notice>
              An account is a key. There is no sign-up, no email and no password reset by anyone. If you lose the 24 words and forget your passphrase, nobody can help — that is the point.
            </Notice>
            <p class="text-xs text-muted">Ledger support: later.</p>
          </div>
        </Show>

        <Show when={step().s === "words"}>
          {(_) => {
            const st = step() as Extract<Step, { s: "words" }>;
            return (
              <div class="flex flex-col gap-4">
                <h1 class="text-xl font-semibold">Your 24 words</h1>
                <p class="text-sm text-muted">
                  Write them down on paper, in order. They are shown once. Never type them into a website, never photograph them, never store them in the cloud. The app will not copy them to the clipboard.
                </p>
                <ol class="mono grid grid-cols-3 gap-1.5 select-none" data-secret="true">
                  <For each={st.words}>
                    {(w, i) => (
                      <li class="flex items-baseline gap-2 rounded-md border border-border bg-surface px-2.5 py-1.5 text-sm">
                        <span class="w-5 text-right text-xs text-muted">{i() + 1}</span>
                        <span>{w}</span>
                      </li>
                    )}
                  </For>
                </ol>
                <div class="flex justify-between">
                  <Button variant="ghost" onClick={() => setStep({ s: "welcome" })}>
                    Back
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
                <h1 class="text-xl font-semibold">Check three words</h1>
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
                            const t = [...typed()];
                            t[i()] = e.currentTarget.value;
                            setTyped(t);
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
                    Continue
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
                <h1 class="text-xl font-semibold">Restore from 24 words</h1>
                <Field label="Your 24 words, separated by spaces" hint={`${count()} / 24 words`}>
                  <Textarea mono rows={4} value={text()} onInput={(e) => setText(e.currentTarget.value)} autocomplete="off" data-secret="true" placeholder="word word word …" />
                </Field>
                <div class="flex justify-between">
                  <Button variant="ghost" onClick={() => setStep({ s: "welcome" })}>
                    Back
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
                <h1 class="text-xl font-semibold">Is this your address?</h1>
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

        <Show when={step().s === "passphrase"}>
          {(_) => {
            const st = step() as Extract<Step, { s: "passphrase" }>;
            const [p1, setP1] = createSignal("");
            const [p2, setP2] = createSignal("");
            const [label, setLabel] = createSignal("");
            const ok = () => p1().length >= 8 && p1() === p2();
            const go = async () => {
              setError(null);
              setBusy(true);
              try {
                const info =
                  st.mode === "create"
                    ? await ipc.onboardingCreate(p1(), label())
                    : await ipc.onboardingRestore(st.mnemonic ?? "", p1(), label());
                await store.refreshStatus();
                setStep({ s: "hello", info });
              } catch (e) {
                fail(e);
              } finally {
                setBusy(false);
              }
            };
            return (
              <div class="flex flex-col gap-4">
                <h1 class="text-xl font-semibold">Set a passphrase for this PC</h1>
                <p class="text-sm text-muted">It encrypts your keys on this computer (Argon2id, 64 MiB). Forgot it? Restore from the 24 words — there is no other reset.</p>
                <Field label="Passphrase" hint="At least 8 characters.">
                  <Input type="password" value={p1()} onInput={(e) => setP1(e.currentTarget.value)} autocomplete="new-password" />
                </Field>
                <Field label="Repeat" error={p2() && p1() !== p2() ? "Does not match" : undefined}>
                  <Input type="password" value={p2()} onInput={(e) => setP2(e.currentTarget.value)} autocomplete="new-password" onKeyDown={(e) => e.key === "Enter" && ok() && void go()} />
                </Field>
                <Field label="Name this device" hint="Shown next to messages you send from here; registered on chain as a device label.">
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
                setStep({ s: "username", info: st.info });
              } catch (e) {
                fail(e);
              } finally {
                setBusy(false);
              }
            };
            return (
              <div class="flex flex-col gap-4">
                <div class="flex items-center gap-2">
                  <Fingerprint size={20} />
                  <h1 class="text-xl font-semibold">Windows Hello (optional)</h1>
                </div>
                <p class="text-sm text-muted">Unlock day to day with your face, fingerprint or PIN. Hello wraps your passphrase behind the Windows security chip; it never sees the 24 words.</p>
                <Show when={store.status()?.hello_available} fallback={<Notice>Windows Hello is not set up on this PC. You can enable it later in Settings → Security.</Notice>}>
                  <Field label="Confirm your passphrase to enrol">
                    <Input type="password" value={pass()} onInput={(e) => setPass(e.currentTarget.value)} autocomplete="current-password" />
                  </Field>
                </Show>
                <div class="flex justify-between">
                  <Button variant="ghost" onClick={() => setStep({ s: "username", info: st.info })}>
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

        <Show when={step().s === "username"}>
          {(_) => {
            const st = step() as Extract<Step, { s: "username" }>;
            return (
              <div class="flex flex-col gap-4">
                <div class="flex items-center gap-2">
                  <AtSign size={20} />
                  <h1 class="text-xl font-semibold">Optional: an @username</h1>
                </div>
                <p class="text-sm text-muted">
                  People can always find you by your address. A username is registered on chain for 1 HASH, valid about a year (7,884,000 blocks) with a 30-day grace period. You can do it later from Wallet → Usernames, once your account has HASH.
                </p>
                <div class="card p-4">
                  <p class="mb-1 text-xs text-muted">Your address</p>
                  <Mono text={st.info.address} full copy class="text-sm" />
                </div>
                <div class="flex justify-end">
                  <Button onClick={() => setStep({ s: "connecting", info: st.info })}>Continue</Button>
                </div>
              </div>
            );
          }}
        </Show>

        <Show when={step().s === "connecting"}>
          {(_) => {
            const st = step() as Extract<Step, { s: "connecting" }>;
            const [snap, { refetch }] = createResource(() => ipc.netSnapshot().catch(() => null));
            onMount(() => {
              const t = setInterval(() => void refetch(), 1000);
              setTimeout(() => clearInterval(t), 60_000);
            });
            return (
              <div class="flex flex-col gap-4">
                <div class="flex items-center gap-2">
                  <NetIcon size={20} />
                  <h1 class="text-xl font-semibold">Connecting to the network…</h1>
                </div>
                <p class="text-sm text-muted">Dialling the bootstrap nodes compiled into the app and completing the Hashgram handshake (genesis hash, chain id, protocol). Nodes appear as they verify.</p>
                <div class="card min-h-24 p-3">
                  <Show when={snap()} fallback={<Skeleton lines={2} />}>
                    {(n) => (
                      <Show when={n().peers.length} fallback={<p class="text-xs text-muted">No node has completed the handshake yet. If this takes more than 10 s, allow outbound UDP and TCP 26670 in Windows Firewall.</p>}>
                        <ul class="mono space-y-1 text-xs">
                          <For each={n().peers}>
                            {(p) => (
                              <li class="flex items-center gap-2">
                                <ShieldCheck size={12} class={p.verified ? "" : "opacity-30"} />
                                <span class="truncate">{p.peer_id}</span>
                                <span class="text-muted">{p.roles.join(",")}</span>
                              </li>
                            )}
                          </For>
                        </ul>
                      </Show>
                    )}
                  </Show>
                </div>
                <p class="text-xs text-muted">
                  Your device key will be registered on chain (<span class="mono">MsgCreateIdentity</span> for a new identity, <span class="mono">MsgAddDevice</span> for an existing one) from Wallet → Identity once your account has HASH for the fee. Only public keys go on chain.
                </p>
                <div class="flex justify-end">
                  <Button onClick={props.onDone}>Open Hashgram</Button>
                </div>
              </div>
            );
          }}
        </Show>
      </div>
    </div>
  );
}
