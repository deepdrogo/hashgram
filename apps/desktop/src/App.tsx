import { createSignal, onMount, Show, Switch, Match, lazy } from "solid-js";
import { HashRouter, Route, useNavigate } from "@solidjs/router";
import { store } from "./lib/store";
import { ipc, on } from "./lib/ipc";
import { Splash } from "./components/Splash";
import { Shell } from "./components/Shell";
import { Onboarding } from "./routes/Onboarding";
import { Lock } from "./routes/Lock";
import { Home } from "./routes/Home";
import { Wallet } from "./routes/wallet/Wallet";
import { Messages } from "./routes/Messages";

const Founder = lazy(() => import("./routes/Founder").then((m) => ({ default: m.Founder })));
const Supply = lazy(() => import("./routes/Supply").then((m) => ({ default: m.Supply })));
const Network = lazy(() => import("./routes/Network").then((m) => ({ default: m.Network })));
const Settings = lazy(() => import("./routes/Settings").then((m) => ({ default: m.Settings })));
const Help = lazy(() => import("./routes/Help").then((m) => ({ default: m.Help })));
const Earn = lazy(() => import("./routes/Earn").then((m) => ({ default: m.Earn })));
const Profile = lazy(() => import("./routes/Profile").then((m) => ({ default: m.Profile })));
const Feed = lazy(() => import("./routes/Feed").then((m) => ({ default: m.Feed })));
const Reels = lazy(() => import("./routes/Reels").then((m) => ({ default: m.Reels })));
const Channels = lazy(() => import("./routes/Channels").then((m) => ({ default: m.Channels })));
const Calls = lazy(() => import("./routes/Calls").then((m) => ({ default: m.Calls })));

type Phase = "boot" | "splash" | "onboarding" | "locked" | "app" | "failed";

export function App() {
  const [phase, setPhase] = createSignal<Phase>("boot");
  const [bootError, setBootError] = createSignal<string>("");
  const start = performance.now();

  const decide = (fresh: boolean) => {
    const s = store.status();
    if (!s) return;
    if (!s.vault_exists) setPhase(fresh ? "splash" : "onboarding");
    else if (!s.unlocked) setPhase("locked");
    else setPhase("app");
  };

  onMount(async () => {
    // Never a silent black window: if the backend does not answer, say so.
    const watchdog = setTimeout(() => {
      if (phase() === "boot") {
        setBootError("The application core did not answer within 8 seconds.");
        setPhase("failed");
        void ipc.perfMark("ui:boot-timeout", 8_000_000);
      }
    }, 8000);
    let s = null;
    try {
      s = await store.refreshStatus();
      if (!s) throw new Error("app_status returned nothing");
    } catch (e) {
      clearTimeout(watchdog);
      setBootError(String(e));
      setPhase("failed");
      return;
    }
    clearTimeout(watchdog);
    void store.refreshSettings();
    void store.refreshNet();
    void store.refreshHealth(false);
    void store.refreshPending();
    await store.wire();
    document.documentElement.dataset.reducedMotion = store.settings()?.appearance.reduced_motion ? "true" : "false";
    decide(!!s && !s.vault_exists);
    // Cold start → interactive, in microseconds, for the Performance panel.
    void ipc.perfMark("ui:interactive", Math.round((performance.now() - start) * 1000));
    await on("session:locked", () => setPhase("locked"));
    // Frame sampling for the 60 fps budget: measure rAF deltas for a while.
    let last = performance.now();
    let n = 0;
    const sample = (t: number) => {
      const d = t - last;
      last = t;
      // rAF timestamps can precede a performance.now() taken just before.
      if (n++ % 30 === 0 && d > 0) void ipc.perfMark("ui:frame", Math.round(d * 1000));
      if (n < 1800) requestAnimationFrame(sample);
    };
    requestAnimationFrame(sample);
  });

  return (
    <Switch>
      <Match when={phase() === "boot"}>
        <div class="h-full bg-bg" />
      </Match>
      <Match when={phase() === "failed"}>
        <div class="flex h-full items-center justify-center p-8">
          <div class="card max-w-md p-6">
            <h1 class="text-lg font-semibold">Hashgram could not start</h1>
            <p class="mt-2 text-sm text-muted selectable">{bootError()}</p>
            <p class="mt-3 text-xs text-muted">Restart the application. If this repeats, export logs from the data folder and report it.</p>
            <button type="button" class="btn-secondary mt-4" onClick={() => location.reload()}>
              Retry
            </button>
          </div>
        </div>
      </Match>
      <Match when={phase() === "splash"}>
        <Splash onDone={() => setPhase("onboarding")} />
      </Match>
      <Match when={phase() === "onboarding"}>
        <Onboarding
          onDone={async () => {
            await store.refreshStatus();
            decide(false);
          }}
        />
      </Match>
      <Match when={phase() === "locked"}>
        <Lock
          onUnlocked={() => {
            void store.refreshHealth(false);
            void store.refreshPending();
            setPhase("app");
          }}
        />
      </Match>
      <Match when={phase() === "app"}>
        <HashRouter root={(p) => <Shell>{p.children}<DeepLinks /></Shell>}>
          <Route path="/" component={Home} />
          <Route path="/messages/:group?" component={Messages} />
          <Route path="/feed" component={Feed} />
          <Route path="/reels" component={Reels} />
          <Route path="/channels/:id?" component={Channels} />
          <Route path="/calls" component={Calls} />
          <Route path="/wallet/*" component={Wallet} />
          <Route path="/earn" component={Earn} />
          <Route path="/founder" component={Founder} />
          <Route path="/supply" component={Supply} />
          <Route path="/network" component={Network} />
          <Route path="/settings" component={Settings} />
          <Route path="/help" component={Help} />
          <Route path="/profile/:address" component={Profile} />
          <Route path="*" component={Home} />
        </HashRouter>
      </Match>
    </Switch>
  );
}

/** hashgram:// links: address → profile, @name → search, post/channel later. */
function DeepLinks() {
  const navigate = useNavigate();
  onMount(() => {
    void on("deep-link", ({ url }) => {
      const rest = url.replace(/^hashgram:\/\//, "").replace(/\/$/, "");
      if (/^hash1/.test(rest)) navigate(`/profile/${rest}`);
      else if (rest.startsWith("@")) void ipc.searchResolve(rest).then((r) => r.kind === "username" && navigate(`/profile/${r.address}`));
      else if (rest.startsWith("channel/")) navigate(`/channels/${rest.slice(8)}`);
      else store.toast(`Unrecognised link: ${url}`, "error");
    });
  });
  return <Show when={false}>{null}</Show>;
}
