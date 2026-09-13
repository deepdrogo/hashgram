import { createSignal, onMount, Show, Switch, Match, lazy } from "solid-js";
import { HashRouter, Route, Navigate, useNavigate, useParams } from "@solidjs/router";
import { store } from "./lib/store";
import { ipc, on } from "./lib/ipc";
import { Splash } from "./components/Splash";
import { Shell } from "./components/Shell";
import { Onboarding } from "./routes/Onboarding";
import { Lock } from "./routes/Lock";
import { MailRoute } from "./routes/mail/Mail";

const Drive = lazy(() => import("./routes/drive/Drive").then((m) => ({ default: m.DriveRoute })));
const Feed = lazy(() => import("./routes/feed/Feed").then((m) => ({ default: m.FeedRoute })));
const Explore = lazy(() => import("./routes/Explore").then((m) => ({ default: m.ExploreRoute })));
const Me = lazy(() => import("./routes/Me").then((m) => ({ default: m.MeRoute })));
const People = lazy(() => import("./routes/People").then((m) => ({ default: m.PeopleRoute })));
const Spaces = lazy(() => import("./routes/spaces/Spaces").then((m) => ({ default: m.SpacesRoute })));
const Earn = lazy(() => import("./routes/Earn").then((m) => ({ default: m.EarnRoute })));
const Wallet = lazy(() => import("./routes/wallet/Wallet").then((m) => ({ default: m.WalletRoute })));
const Network = lazy(() => import("./routes/Network").then((m) => ({ default: m.NetworkRoute })));
const Settings = lazy(() => import("./routes/Settings").then((m) => ({ default: m.SettingsRoute })));
const Help = lazy(() => import("./routes/Help").then((m) => ({ default: m.Help })));

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
    await store.refreshSettings();
    await store.wire();
    void store.refreshSync();
    void store.refreshPending();
    if (s.unlocked) {
      void store.refreshCounts();
      void store.refreshIdentity();
    }
    decide(!!s && !s.vault_exists);
    void ipc.perfMark("ui:interactive", Math.round((performance.now() - start) * 1000));
    await on("session:locked", () => setPhase("locked"));
    await on("session:unlocked", () => {
      void store.refreshCounts();
      void store.refreshIdentity();
      setPhase("app");
    });
    let last = performance.now();
    let n = 0;
    const sample = (t: number) => {
      const d = t - last;
      last = t;
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
            void store.refreshCounts();
            void store.refreshPending();
            setPhase("app");
          }}
        />
      </Match>
      <Match when={phase() === "app"}>
        <HashRouter
          root={(p) => (
            <Shell>
              {p.children}
              <DeepLinks />
            </Shell>
          )}
        >
          <Route path="/" component={() => <Navigate href="/mail/inbox" />} />
          <Route path="/mail/:folder?/:id?" component={MailRoute} />
          <Route path="/drive/:parent?" component={Drive} />
          <Route path="/hashwall/:tab?/:id?" component={Feed} />
          <Route path="/feed/:tab?/:id?" component={FeedRedirect} />
          <Route path="/explore/:section?/:arg?" component={Explore} />
          <Route path="/me" component={Me} />
          <Route path="/people/:address?" component={People} />
          <Route path="/spaces/:id?/:tab?" component={Spaces} />
          <Route path="/earn/:tab?" component={Earn} />
          <Route path="/wallet/:tab?" component={Wallet} />
          <Route path="/network" component={Network} />
          <Route path="/settings/:tab?" component={Settings} />
          <Route path="/help/:slug?" component={Help} />
          <Route path="*" component={() => <Navigate href="/mail/inbox" />} />
        </HashRouter>
      </Match>
    </Switch>
  );
}

/** `/feed/…` → `/hashwall/…`, same tab and post. */
function FeedRedirect() {
  const params = useParams<{ tab?: string; id?: string }>();
  const target = () => `/hashwall${params.tab ? `/${params.tab}` : ""}${params.id ? `/${params.id}` : ""}`;
  return <Navigate href={target()} />;
}

/** hashgram:// links: mail/<id>, space/<id>, drive/<id>, user/<addr|@name>, wall/<id>, post/<id>, tag/<name>. */
function DeepLinks() {
  const navigate = useNavigate();
  onMount(() => {
    void on("deep-link", ({ url }) => {
      const rest = url.replace(/^hashgram:\/\//, "").replace(/\/$/, "");
      const [head, ...tail] = rest.split("/");
      const arg = decodeURIComponent(tail.join("/"));
      if (head === "mail" && arg) navigate(`/mail/inbox/${arg}`);
      else if (head === "space" && arg) navigate(`/spaces/${arg}`);
      else if (head === "drive" && arg) navigate(`/drive/?select=${arg}`);
      else if (head === "user" && arg) navigate(arg.startsWith("hash1") ? `/people/${arg}` : `/people?q=${encodeURIComponent(arg)}`);
      else if (head === "wall" && /^[0-9a-f]{64}$/i.test(arg)) navigate(`/hashwall/walls/${arg.toLowerCase()}`);
      else if (head === "post" && /^[0-9a-f]{64}$/i.test(arg)) navigate(`/hashwall/friends/${arg.toLowerCase()}`);
      else if (head === "tag" && arg) navigate(`/explore/posts/${encodeURIComponent(arg.replace(/^#/, ""))}`);
      else if (/^hash1/.test(rest)) navigate(`/people/${rest}`);
      else if (rest.startsWith("@")) navigate(`/people?q=${encodeURIComponent(rest)}`);
      else store.toast(`Unrecognised link: ${url}`, "error");
    });
  });
  return <Show when={false}>{null}</Show>;
}
