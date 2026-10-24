import { useEffect, useRef, useState } from "react";
import type { Account, SwitchStage } from "../api";
import { useApp } from "../store";
import { bestRole, isStale, newestSeason, rankName, ROLE_NAME, statusText } from "../rank";
import { HOVER_DELAY_MS } from "./HoverCard";
import { Icon } from "./Icon";
import { TierIcon, useAvatarUrl } from "./RankEmblem";

export const STAGE_WORDS: Record<SwitchStage, string> = {
  closing_game: "Closing the game",
  closing: "Closing Battle.net",
  restoring: "Restoring login",
  launching: "Starting Battle.net",
  verifying: "Signing in",
  done: "Signed in",
  needs_login: "Log in once",
  error: "Did not switch",
};

const STAGE_FILL: Partial<Record<SwitchStage, number>> = { closing_game: 0.06, closing: 0.16, restoring: 0.34, launching: 0.58, verifying: 0.86 };

/** A muted tint per account so the initials tell cards apart at a glance; derived from the code, stable forever. */
function tint(code: string) {
  let h = 0;
  for (const c of code) h = (h * 31 + c.charCodeAt(0)) % 360;
  return `hsl(${h} 18% 30%)`;
}

export function AccountCard({ account, index, count }: { account: Account; index: number; count: number }) {
  const { code, label, battletag, status } = account;
  const selected = useApp((s) => s.selected === code);
  const signedIn = useApp((s) => s.current === code);
  const switching = useApp((s) => (s.switching?.code === code ? s.switching.stage : null));
  const closingGame = useApp((s) => (s.switching?.code === code ? s.switching.game : undefined));
  const busyElsewhere = useApp((s) => !!(s.switching && s.switching.code !== code) || !!s.importing);
  const note = useApp((s) => s.notes[code]);
  const renaming = useApp((s) => s.renaming === code);
  const { select, switchTo, set } = useApp.getState();

  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => { if (selected) ref.current?.scrollIntoView({ block: "nearest" }); }, [selected]);

  // Rank card: shown after the pointer rests on the card for 250 ms, gone the moment it leaves.
  const hoverTimer = useRef(0);
  const showHover = () => {
    window.clearTimeout(hoverTimer.current);
    hoverTimer.current = window.setTimeout(() => {
      const r = ref.current?.getBoundingClientRect();
      if (r) set({ hover: { code, left: r.left, top: r.top, right: r.right, bottom: r.bottom } });
    }, HOVER_DELAY_MS);
  };
  const hideHover = () => {
    window.clearTimeout(hoverTimer.current);
    if (useApp.getState().hover?.code === code) set({ hover: null });
  };
  useEffect(() => () => window.clearTimeout(hoverTimer.current), []);

  // The progress line holds full width for a moment after "done" before it fades, so the finish is visible.
  const fill = switching ? STAGE_FILL[switching] ?? 0.1 : note?.kind === "ok" && note.text.startsWith("Signed in") ? 1 : 0;

  let stateText = "";
  let stateKind = "";
  if (switching) { stateText = switching === "closing_game" && closingGame ? `Closing ${closingGame}` : STAGE_WORDS[switching]; stateKind = "busy"; }
  else if (note) { stateText = note.text; stateKind = note.kind; }
  else if (status === "needs_login") { stateText = "Log in once"; stateKind = "warn"; }

  const dot = signedIn ? "signed" : status === "needs_login" ? "needs" : "ready";
  const dotTitle = signedIn ? "Signed in now" : status === "needs_login" ? "Needs one login in Battle.net" : "Ready";

  return (
    <div
      ref={ref}
      data-code={code}
      role="option"
      aria-selected={selected}
      aria-label={`${label}${battletag ? ", " + battletag : ""}${signedIn ? ", signed in" : ""}`}
      className={"card acct-card" + (selected ? " selected" : "") + (signedIn ? " signed-in" : "") + (switching ? " busy" : "") + (status === "needs_login" ? " needs" : "")}
      style={{ ["--i" as string]: index }}
      onMouseEnter={showHover}
      onMouseLeave={hideHover}
      onMouseDown={() => select(code)}
      onDoubleClick={() => { if (!renaming) void switchTo(code); }}
      onContextMenu={(e) => { e.preventDefault(); hideHover(); select(code); set({ menu: { code, x: e.clientX, y: e.clientY }, settingsOpen: false }); }}
    >
      <div className="card-top">
        <Avatar code={code} label={label} />
        <span className="card-top-right">
          <RankBadge code={code} />
          <span className={"dot dot-" + dot} title={dotTitle} />
        </span>
      </div>
      {renaming ? <RenameField code={code} label={label} /> : <div className="card-label" title={label}>{label}</div>}
      <div className="card-sub" title={battletag ?? undefined}>{battletag ?? "\u00a0"}</div>
      <div className="card-foot">
        <span className={"card-state " + stateKind} key={stateText}>{stateText}</span>
        {signedIn && !switching && !(note?.kind === "ok") ? (
          <span className="tag-signed">Signed in</span>
        ) : !switching && !signedIn && !note ? (
          <button className="btn-login" disabled={busyElsewhere} tabIndex={-1}
            onMouseDown={(e) => e.stopPropagation()} onClick={() => void switchTo(code)}>
            Log in
          </button>
        ) : null}
      </div>
      <div className={"card-progress" + (fill ? " on" : "")} aria-hidden="true">
        <i style={{ transform: `scaleX(${fill})` }} />
      </div>
      <span className="sr-only">{`Account ${index + 1} of ${count}`}</span>
    </div>
  );
}

/** The Overwatch player icon once the engine has it on disk, else the first letter on the account's tint. */
function Avatar({ code, label }: { code: string; label: string }) {
  const url = useAvatarUrl(code);
  const [broken, setBroken] = useState<string | null>(null);
  if (url && broken !== url) {
    return <img className="avatar avatar-img" src={url} alt="" aria-hidden="true" draggable={false} onError={() => setBroken(url)} />;
  }
  return <div className="avatar" style={{ background: tint(code) }} aria-hidden="true">{(label.trim()[0] ?? "?").toUpperCase()}</div>;
}

function RenameField({ code, label }: { code: string; label: string }) {
  const [v, setV] = useState(label);
  const ref = useRef<HTMLInputElement>(null);
  const done = useRef(false);
  useEffect(() => { ref.current?.focus(); ref.current?.select(); }, []);
  const commit = () => { if (done.current) return; done.current = true; void useApp.getState().rename(code, v); };
  const cancel = () => { done.current = true; useApp.getState().set({ renaming: null }); };
  return (
    <input
      ref={ref}
      className="card-label-input"
      value={v}
      maxLength={40}
      aria-label="Account name"
      onChange={(e) => setV(e.target.value)}
      onMouseDown={(e) => e.stopPropagation()}
      onDoubleClick={(e) => e.stopPropagation()}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === "Enter") commit();
        if (e.key === "Escape") cancel();
      }}
      onBlur={commit}
    />
  );
}

export function AddCard({ index }: { index: number }) {
  const selected = useApp((s) => s.selected === "add");
  const busy = useApp((s) => !!s.switching || !!s.adding || !!s.importing);
  const { select, add } = useApp.getState();
  const ref = useRef<HTMLButtonElement>(null);
  useEffect(() => { if (selected) ref.current?.scrollIntoView({ block: "nearest" }); }, [selected]);
  return (
    <button ref={ref} data-code="add" role="option" aria-selected={selected} tabIndex={-1} disabled={busy}
      className={"card add-card" + (selected ? " selected" : "")} style={{ ["--i" as string]: index }}
      onMouseDown={() => select("add")} onClick={() => void add()}>
      <span className="add-ring"><Icon name="plus" size={18} /></span>
      <span className="add-text">Add account</span>
    </button>
  );
}

/** Best role rank as a small crest + words, so the grid reads like a ladder. A guess (placements) is faded. */
function RankBadge({ code }: { code: string }) {
  const info = useApp((s) => s.ranks[code]);
  const stale = useApp((s) => isStale(s.ranks[code], newestSeason(s.ranks)));
  if (!info) return null;
  if (info.privateProfile) return <span className="rank-badge muted" title="Profile is private"><Icon name="lock" size={12} />Private</span>;
  const best = bestRole(info);
  // Nothing to show and the last lookup did not find the profile (OverFast's cached 404) or failed.
  if (!best && info.status === "notFound") return <span className="rank-badge muted" title={statusText(info)}><Icon name="search" size={12} />Not found</span>;
  if (!best && info.status === "error") return <span className="rank-badge muted" title={statusText(info)}><Icon name="alert" size={12} />Error</span>;
  if (best?.predicted) {
    return (
      <span className="rank-badge predicted" title={`${ROLE_NAME[best.role]} in placements, predicted ${rankName(best.rank)}`}>
        <span className="is-predicted"><TierIcon tier={best.rank.tier} size={20} /></span>{rankName(best.rank)}
      </span>
    );
  }
  if (best) {
    return (
      <span className={"rank-badge" + (stale ? " stale" : "")} title={`Best role: ${ROLE_NAME[best.role]}, ${rankName(best.rank)}${stale ? ", last season" : ""}`}>
        <TierIcon tier={best.rank.tier} size={20} />{rankName(best.rank)}
      </span>
    );
  }
  return <span className="rank-badge muted">Unranked</span>;
}
