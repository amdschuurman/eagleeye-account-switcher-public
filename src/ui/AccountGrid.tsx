import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import { useApp, visibleAccounts } from "../store";
import { AccountCard, AddCard } from "./AccountCard";
import { Icon } from "./Icon";

/** Columns the grid is drawing right now (auto-fill), so Up and Down move a whole row. */
function columns(el: HTMLElement | null) {
  if (!el) return 1;
  return Math.max(1, getComputedStyle(el).gridTemplateColumns.split(" ").filter(Boolean).length);
}

// Layout animation when the visible set or its order changes (filter, sort, category, move): cards that stay slide to
// their new spot (FLIP: measure before, measure after, animate the difference with a transform), new cards fade and rise
// with a small stagger, cards that leave fade out quickly as a detached copy. Reads all rects first, then writes; skipped
// above FLIP_MAX cards and under reduced motion.
const FLIP_MAX = 60;
const FLIP_MS = 220;
const LEAVE_MS = 140;
const EASE = "cubic-bezier(0.22, 1, 0.36, 1)"; // --ease in app.css

interface Spot { el: HTMLElement; x: number; y: number; w: number; h: number }
interface Before { spots: Map<string, Spot>; ghosts: Map<string, HTMLElement> }

const reducedMotion = () => window.matchMedia("(prefers-reduced-motion: reduce)").matches;

/** Every card's box in the grid's own content coordinates (so scrolling between two reads does not count as a move). */
function measure(grid: HTMLElement): Map<string, Spot> {
  const g = grid.getBoundingClientRect();
  const out = new Map<string, Spot>();
  grid.querySelectorAll<HTMLElement>(":scope > [data-code]").forEach((el) => {
    const r = el.getBoundingClientRect();
    out.set(el.dataset.code!, { el, x: r.left - g.left + grid.scrollLeft, y: r.top - g.top + grid.scrollTop, w: r.width, h: r.height });
  });
  return out;
}

function useFlip(gridRef: React.RefObject<HTMLDivElement>, codes: string[]) {
  const order = codes.join(",");
  const committed = useRef(order);
  const before = useRef<Before | null>(null);
  // During the render that changes the order the DOM still shows the old layout: take the "before" picture now, and a
  // copy of each card that is about to go (React removes it before any effect could see it).
  if (order !== committed.current && !before.current && gridRef.current && codes.length <= FLIP_MAX && !reducedMotion()) {
    const spots = measure(gridRef.current);
    const keep = new Set([...codes, "add"]);
    const ghosts = new Map<string, HTMLElement>();
    spots.forEach((sp, code) => { if (!keep.has(code)) ghosts.set(code, sp.el.cloneNode(true) as HTMLElement); });
    before.current = { spots, ghosts };
  }
  useLayoutEffect(() => {
    const was = before.current;
    before.current = null;
    committed.current = order;
    const grid = gridRef.current;
    if (!was || !grid) return;
    grid.querySelectorAll<HTMLElement>(":scope > [data-code]").forEach((el) => el.getAnimations().forEach((a) => { if (a.id === "flip") a.cancel(); }));
    const after = measure(grid); // the only layout read after the change; everything below only writes
    let fresh = 0;
    after.forEach((a, code) => {
      const b = was.spots.get(code);
      if (b) {
        const dx = b.x - a.x, dy = b.y - a.y;
        if (Math.abs(dx) >= 1 || Math.abs(dy) >= 1) {
          a.el.animate([{ transform: `translate(${dx}px, ${dy}px)` }, { transform: "none" }], { duration: FLIP_MS, easing: EASE, id: "flip" });
        }
      } else {
        a.el.animate([{ opacity: 0, transform: "translateY(6px) scale(0.98)" }, { opacity: 1, transform: "none" }],
          { duration: FLIP_MS, delay: Math.min(fresh++, 8) * 28, easing: EASE, fill: "backwards", id: "flip" });
      }
    });
    was.ghosts.forEach((g, code) => {
      const b = was.spots.get(code)!;
      g.removeAttribute("data-code");
      g.removeAttribute("role");
      g.setAttribute("aria-hidden", "true");
      g.classList.add("flip-ghost");
      Object.assign(g.style, { left: `${b.x}px`, top: `${b.y}px`, width: `${b.w}px`, height: `${b.h}px` });
      grid.appendChild(g);
      const anim = g.animate([{ opacity: 1, transform: "none" }, { opacity: 0, transform: "scale(0.96)" }], { duration: LEAVE_MS, easing: EASE, fill: "forwards" });
      anim.onfinish = () => g.remove();
    });
  }, [order, gridRef]);
}

export function AccountGrid() {
  const accounts = useApp(useShallow(visibleAccounts));
  const narrowed = useApp((s) => s.category !== "all" || !!s.filter);
  const gridRef = useRef<HTMLDivElement>(null);
  useFlip(gridRef, accounts.map((a) => a.code));
  // The staggered CSS entrance plays once per page visit; after it, appearing cards are animated by useFlip instead.
  const [settled, setSettled] = useState(false);
  useEffect(() => { const t = window.setTimeout(() => setSettled(true), 700); return () => window.clearTimeout(t); }, []);

  // Keyboard: arrows move the selection, Enter logs in (or adds), F2 renames, Delete asks to remove.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const s = useApp.getState();
      if (e.altKey || e.ctrlKey || e.metaKey || s.page !== "battlenet") return; // Alt+arrows are page history (App.tsx)
      if (s.adding || s.renaming || s.confirmRemove || s.settingsOpen || s.menu || s.editRanks || s.filterOpen || s.sortOpen) return;
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA")) return;
      const ids = [...visibleAccounts(s).map((a) => a.code), "add"];
      let i = Math.max(0, ids.indexOf(s.selected ?? ""));
      const cols = columns(gridRef.current);
      const moves: Record<string, number> = { ArrowLeft: -1, ArrowRight: 1, ArrowUp: -cols, ArrowDown: cols };
      if (e.key in moves) {
        e.preventDefault();
        const j = i + moves[e.key];
        if (j >= 0 && j < ids.length) i = j;
        else if (e.key === "ArrowDown") i = ids.length - 1;
        else if (e.key === "ArrowUp") i = 0;
        s.select(ids[i]);
      } else if (e.key === "Home") { e.preventDefault(); s.select(ids[0]); }
      else if (e.key === "End") { e.preventDefault(); s.select(ids[ids.length - 1]); }
      else if (e.key === "Enter" && s.selected) {
        e.preventDefault();
        if (s.selected === "add") void s.add(); else void s.switchTo(s.selected);
      } else if (e.key === "F2" && s.selected && s.selected !== "add") { e.preventDefault(); s.set({ renaming: s.selected }); }
      else if (e.key === "Delete" && s.selected && s.selected !== "add" && !s.switching) { e.preventDefault(); s.set({ confirmRemove: s.selected }); }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  return (
    <div className={"grid" + (settled ? " settled" : "")} ref={gridRef} role="listbox" aria-label="Battle.net accounts">
      {accounts.map((a, i) => <AccountCard key={a.code} account={a} index={i} count={accounts.length} />)}
      {narrowed && accounts.length === 0 && <NoMatch />}
      <AddCard index={accounts.length} />
    </div>
  );
}

/** TcNo accounts found and nothing saved yet: one quiet line, with its own progress while the import runs. */
export function ImportBanner() {
  const tcno = useApp((s) => s.tcno);
  const empty = useApp((s) => s.accounts.length === 0);
  const dismissed = useApp((s) => s.importDismissed);
  const importing = useApp((s) => s.importing);
  const { importTcno, set } = useApp.getState();
  if (!importing && (!tcno || !empty || dismissed)) return null;
  const frac = importing && importing.total ? importing.done / importing.total : 0;
  return (
    <div className={"banner" + (importing ? " running" : "")} role="status">
      <span className="banner-ic"><Icon name="download" size={16} /></span>
      {importing ? (
        <span className="banner-text">
          Importing from TcNo, {Math.min(importing.total, importing.done + (importing.running ? 1 : 0)) || 1} of {importing.total}
          <span className="banner-sub">{importing.label ? `Signing in ${importing.label} once to save it` : "Closing Battle.net"}</span>
        </span>
      ) : (
        <span className="banner-text">
          Found {tcno} Battle.net {tcno === 1 ? "account" : "accounts"} in TcNo Account Switcher.
          <span className="banner-sub">Each one is signed in once to save it here. Takes about 5 seconds per account.</span>
        </span>
      )}
      {!importing && (
        <>
          <button className="btn-orange" onClick={() => void importTcno()}>Import {tcno} {tcno === 1 ? "account" : "accounts"}</button>
          <button className="icon-btn" aria-label="Not now" title="Not now" onClick={() => set({ importDismissed: true })}><Icon name="close" size={14} /></button>
        </>
      )}
      <div className="banner-progress" aria-hidden="true"><i style={{ transform: `scaleX(${frac})` }} /></div>
    </div>
  );
}

/** Nothing saved yet: what the grid means, plus the one-step way to save the account Battle.net is already signed in to. */
export function EmptyHint() {
  const empty = useApp((s) => s.accounts.length === 0 && s.loaded);
  const current = useApp((s) => s.current);
  const saving = useApp((s) => s.savingCurrent);
  const importing = useApp((s) => !!s.importing);
  const save = useApp((s) => s.saveCurrent);
  if (!empty || importing) return null;
  return (
    <div className="empty-hint">
      <p>Save each Battle.net account once. After that, one click signs it in, no password.</p>
      {current && (
        <button className="btn-grey" disabled={saving} onClick={() => void save()}>
          {saving ? "Saving the signed-in account" : "Save the account that is signed in now"}
        </button>
      )}
    </div>
  );
}

function NoMatch() {
  const set = useApp((s) => s.set);
  return (
    <div className="no-match">
      <span className="no-match-ic"><Icon name="filter" size={15} /></span>
      <span>No account fits this filter.</span>
      <button className="link-btn" onClick={() => set({ filter: null, category: "all" })}>Show all accounts</button>
    </div>
  );
}
