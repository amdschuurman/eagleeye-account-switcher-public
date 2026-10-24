// The app's two pages and the switch between them. "platforms" lists every platform the switcher handles (data-driven:
// PLATFORMS below, one entry per platform, so Steam is one more entry later); "battlenet" is the account grid. Back and
// forward work like a browser (store.navigate / back / forward); a page change is a short crossfade with a slight slide
// in the direction of travel, transform and opacity only. The emblems are optional (src/emblems.ts): without them the
// platform tile shows the platform's initial and the heading shows no emblem, never a broken image.
import { useEffect, useRef, useState } from "react";
import { battlenetEmblem, overwatchEmblem } from "../emblems";
import { useApp, type Page } from "../store";
import { AccountGrid, EmptyHint, ImportBanner } from "./AccountGrid";
import { Icon } from "./Icon";
import { Toolbar } from "./Toolbar";

interface Platform {
  id: string;
  name: string;
  emblem?: string;
  games: Array<{ name: string; emblem?: string }>;
  page: Page;
}

export const PLATFORMS: Platform[] = [
  { id: "battlenet", name: "Battle.net", emblem: battlenetEmblem, games: [{ name: "Overwatch", emblem: overwatchEmblem }], page: "battlenet" },
];

/** Accounts saved and the signed-in one, per platform. Only Battle.net has an engine today. */
function useSummary(id: string): { count: number; signedIn: string | null } {
  const count = useApp((s) => (id === "battlenet" ? s.accounts.length : 0));
  const signedIn = useApp((s) => (id === "battlenet" ? s.accounts.find((a) => a.code === s.current)?.label ?? null : null));
  return { count, signedIn };
}

function PlatformCard({ p, index, autoFocus }: { p: Platform; index: number; autoFocus: boolean }) {
  const { count, signedIn } = useSummary(p.id);
  const navigate = useApp((s) => s.navigate);
  const ref = useRef<HTMLButtonElement>(null);
  // Enter opens the platform right away (focus without a ring after a mouse click: :focus-visible decides).
  useEffect(() => { if (autoFocus) ref.current?.focus({ preventScroll: true }); }, [autoFocus]);
  const games = p.games.map((g) => g.name).join(", ");
  return (
    <button ref={ref} className="card platform-card" style={{ ["--i" as string]: index }} onClick={() => navigate(p.page)}
      aria-label={`${p.name}, ${games}, ${count} ${count === 1 ? "account" : "accounts"}${signedIn ? ", signed in as " + signedIn : ""}`}>
      <span className="pf-top">
        <span className="pf-tile">
          {p.emblem ? <img src={p.emblem} alt="" className="pf-emblem" draggable={false} /> : <span className="pf-initial" aria-hidden="true">{p.name[0]}</span>}
          {p.games.map((g) => g.emblem && <img key={g.name} src={g.emblem} alt="" title={g.name} className="pf-game" draggable={false} />)}
        </span>
        <Icon name="arrowRight" size={16} className="pf-go" />
      </span>
      <span className="pf-name">{p.name}</span>
      <span className="pf-sub">{games}, {count} {count === 1 ? "account" : "accounts"}</span>
      <span className="pf-foot">
        {signedIn ? <><span className="dot dot-signed" /><span>Signed in as <b>{signedIn}</b></span></> : <span className="pf-none">Not signed in</span>}
      </span>
    </button>
  );
}

function PlatformsPage({ active }: { active: boolean }) {
  return (
    <>
      <div className="heading"><h1>Platforms</h1><span className="heading-sub">Pick where to switch accounts</span></div>
      <div className="platform-grid">
        {PLATFORMS.map((p, i) => <PlatformCard key={p.id} p={p} index={i} autoFocus={active && i === 0} />)}
      </div>
    </>
  );
}

function BattlenetPage() {
  const loaded = useApp((s) => s.loaded);
  const current = useApp((s) => s.accounts.find((a) => a.code === s.current));
  const hasAccounts = useApp((s) => s.accounts.length > 0);
  return (
    <>
      <div className="heading">
        {battlenetEmblem && <img src={battlenetEmblem} alt="" className="heading-emblem" draggable={false} />}
        <h1>Battle.net</h1>
        {overwatchEmblem && <img src={overwatchEmblem} alt="Overwatch" title="Overwatch" className="heading-emblem game" draggable={false} />}
        <span className="heading-sub">{current ? <>Signed in as <b>{current.label}</b></> : hasAccounts ? "Double-click an account to log in" : ""}</span>
      </div>
      <Toolbar />
      <ImportBanner />
      <EmptyHint />
      {loaded && <AccountGrid />}
    </>
  );
}

const ORDER: Page[] = ["platforms", "battlenet"]; // fixed DOM order: a page change never moves a node (z-index stacks them)
const LEAVE_MS = 160;

export function Pages() {
  const page = useApp((s) => s.page);
  const dir = useApp((s) => s.navDir);
  // The page being left stays mounted for a moment under the new one. Derived during render (not in an effect) so the
  // old page is never unmounted and mounted again in between.
  const [shown, setShown] = useState<{ page: Page; leaving: Page | null }>({ page, leaving: null });
  const navigated = useRef(false);
  if (shown.page !== page) {
    navigated.current = true;
    setShown({ page, leaving: shown.page });
  }
  useEffect(() => {
    if (!shown.leaving) return;
    const t = window.setTimeout(() => setShown((s) => ({ ...s, leaving: null })), LEAVE_MS);
    return () => window.clearTimeout(t);
  }, [shown]);
  const leaving = shown.leaving !== page ? shown.leaving : null;
  return (
    <div className="pages" style={{ ["--dir" as string]: dir }}>
      {ORDER.filter((p) => p === page || p === leaving).map((p) => (
        <section key={p} className={"page" + (p === leaving ? " leave" : navigated.current ? " enter" : "")} aria-hidden={p === leaving || undefined}
          aria-label={p === "platforms" ? "Platforms" : "Battle.net accounts"}>
          {p === "platforms" ? <PlatformsPage active={p === page} /> : <BattlenetPage />}
        </section>
      ))}
    </div>
  );
}
