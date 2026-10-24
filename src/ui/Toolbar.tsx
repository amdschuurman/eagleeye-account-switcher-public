// Under the "Battle.net" heading: category chips (All, Main, Smurf with counts), the active rank filter as a clearable
// chip, and on the right the sort menu and the rank filter popover.
import { Fragment, useEffect, useRef } from "react";
import type { Role } from "../api";
import { categoryOf, useApp, type RankFilter, type SortKey } from "../store";
import { fromScore, MAX_SCORE, rankName, ROLE_NAME, ROLES, score } from "../rank";
import { Fade } from "./Fade";
import { Icon } from "./Icon";
import { RoleIcon, TierIcon } from "./RankEmblem";

const SORTS: Array<[SortKey, string]> = [
  ["manual", "Manual"], ["best", "Highest rank"], ["tank", "Tank"], ["damage", "Damage"], ["support", "Support"], ["open", "Open Queue"],
];

export function Toolbar() {
  const accounts = useApp((s) => s.accounts);
  const category = useApp((s) => s.category);
  const filter = useApp((s) => s.filter);
  const sort = useApp((s) => s.sort);
  const sortOpen = useApp((s) => s.sortOpen);
  const filterOpen = useApp((s) => s.filterOpen);
  const set = useApp((s) => s.set);
  if (!accounts.length) return null;
  const main = accounts.filter((a) => categoryOf(a) === "main").length;
  const chips: Array<["all" | "main" | "smurf", string, number]> = [["all", "All", accounts.length], ["main", "Main", main], ["smurf", "Smurf", accounts.length - main]];
  return (
    <div className="toolbar">
      <div className="chips" role="tablist" aria-label="Category">
        {chips.map(([k, name, n]) => (
          <button key={k} role="tab" aria-selected={category === k} className={"chip" + (category === k ? " on" : "")} onClick={() => set({ category: k })}>
            {name}<span className="chip-n" key={n}>{n}</span>
          </button>
        ))}
        {filter && (
          <span className="chip filter-chip" key={JSON.stringify(filter)}>
            {filter.role === "best" ? <Icon name="medal" size={13} /> : <RoleIcon role={filter.role} size={13} />}
            {filterText(filter)}
            <button className="chip-x" aria-label="Clear the rank filter" title="Clear the rank filter" onClick={() => set({ filter: null })}><Icon name="close" size={11} stroke={2.2} /></button>
          </span>
        )}
      </div>
      <div className="tools">
        <div className="tool-wrap">
          <button className={"tool-btn" + (sortOpen ? " open" : "") + (sort !== "manual" ? " active" : "")} aria-haspopup="menu" aria-expanded={sortOpen}
            onClick={() => set({ sortOpen: !sortOpen, filterOpen: false, menu: null })}>
            <Icon name="sort" size={14} /><span className="tool-label">Sort</span><b>{SORTS.find(([k]) => k === sort)![1]}</b><Icon name="chevronDown" size={12} />
          </button>
          <SortMenu />
        </div>
        <div className="tool-wrap">
          <button className={"tool-btn" + (filterOpen ? " open" : "") + (filter ? " active" : "")} aria-haspopup="dialog" aria-expanded={filterOpen}
            onClick={() => set({ filterOpen: !filterOpen, sortOpen: false, menu: null })}>
            <Icon name="filter" size={14} /><span className="tool-label">Filter</span>{filter && <span className="tool-dot" />}
          </button>
          <FilterPopover />
        </div>
      </div>
    </div>
  );
}

function filterText(f: RankFilter) {
  const lo = Math.min(f.from, f.to), hi = Math.max(f.from, f.to);
  const who = f.role === "best" ? "Best role" : ROLE_NAME[f.role];
  return lo === hi ? `${who}, ${rankName(fromScore(lo))}` : `${who}, ${rankName(fromScore(lo))} to ${rankName(fromScore(hi))}`;
}

/** Close a popover on an outside press or Escape. Its toggle button shares the .tool-wrap, so that click just toggles. */
function useDismiss(open: boolean, close: () => void, wrap: React.RefObject<HTMLElement>) {
  useEffect(() => {
    if (!open) return;
    const down = (e: MouseEvent) => { if (!(e.target instanceof Node && wrap.current?.closest(".tool-wrap")?.contains(e.target))) close(); };
    const key = (e: KeyboardEvent) => { if (e.key === "Escape") close(); };
    window.addEventListener("mousedown", down, true);
    window.addEventListener("keydown", key);
    return () => { window.removeEventListener("mousedown", down, true); window.removeEventListener("keydown", key); };
  }, [open, close, wrap]);
}

function SortMenu() {
  const open = useApp((s) => s.sortOpen);
  const sort = useApp((s) => s.sort);
  const set = useApp((s) => s.set);
  const ref = useRef<HTMLDivElement>(null);
  useDismiss(open, () => set({ sortOpen: false }), ref);
  return (
    <Fade show={open} outMs={120} className="pop-anchor">
      <div ref={ref} className="ctx-menu pop-menu" role="menu" aria-label="Sort">
        {SORTS.map(([k, name], i) => (
          <Fragment key={k}>
            {i === 2 && <div className="ctx-sep" />}
            <button role="menuitemradio" aria-checked={sort === k} className={sort === k ? "checked" : ""} onClick={() => set({ sort: k, sortOpen: false })}>
              <span className="ctx-check">{sort === k && <Icon name="check" size={13} stroke={2.2} />}</span>
              {k === "manual" || k === "best" ? null : <RoleIcon role={k} size={13} />}
              {name}
            </button>
          </Fragment>
        ))}
      </div>
    </Fade>
  );
}

const RANK_OPTIONS = (() => {
  const out: number[] = [];
  for (let s = MAX_SCORE; s >= 0; s--) out.push(s);
  return out;
})();

function RankSelect({ value, onChange, label }: { value: number; onChange: (v: number) => void; label: string }) {
  const r = fromScore(value);
  return (
    <label className="rank-select">
      <span className="rank-select-label">{label}</span>
      <span className="select-wrap">
        <TierIcon tier={r.tier} size={18} />
        <select className="select" value={value} onChange={(e) => onChange(Number(e.target.value))} aria-label={label}>
          {RANK_OPTIONS.map((v) => <option key={v} value={v}>{rankName(fromScore(v))}</option>)}
        </select>
        <Icon name="chevronDown" size={12} />
      </span>
    </label>
  );
}

const DEFAULT_FILTER: RankFilter = { role: "best", from: score({ tier: "gold", division: 5 }), to: score({ tier: "platinum", division: 1 }) };

function FilterPopover() {
  const open = useApp((s) => s.filterOpen);
  const filter = useApp((s) => s.filter);
  const set = useApp((s) => s.set);
  const ref = useRef<HTMLDivElement>(null);
  useDismiss(open, () => set({ filterOpen: false }), ref);
  const f = filter ?? DEFAULT_FILTER;
  // The filter applies live; opening the popover with no filter yet does nothing until something is picked.
  const apply = (p: Partial<RankFilter>) => set({ filter: { ...f, ...p } });
  const roles: Array<Role | "best"> = ["best", ...ROLES];
  return (
    <Fade show={open} outMs={120} className="pop-anchor">
      <div ref={ref} className="filter-pop" role="dialog" aria-label="Rank filter">
        <div className="filter-title">Show accounts ranked</div>
        <div className="seg" role="radiogroup" aria-label="Role">
          {roles.map((r) => (
            <button key={r} role="radio" aria-checked={filter?.role === r} className={"seg-btn" + (filter?.role === r ? " on" : "")} onClick={() => apply({ role: r })}>
              {r === "best" ? <Icon name="medal" size={13} /> : <RoleIcon role={r} size={13} />}
              {r === "best" ? "Best role" : r === "open" ? "Open" : ROLE_NAME[r]}
            </button>
          ))}
        </div>
        <div className="range">
          <RankSelect label="From" value={Math.min(f.from, f.to)} onChange={(v) => apply({ from: v, to: Math.max(v, Math.max(f.from, f.to)) })} />
          <RankSelect label="To" value={Math.max(f.from, f.to)} onChange={(v) => apply({ to: v, from: Math.min(v, Math.min(f.from, f.to)) })} />
        </div>
        <div className="filter-foot">
          <span className="filter-hint">Unranked and private accounts are hidden.</span>
          {filter ? <button className="btn-grey sm" onClick={() => set({ filter: null })}>Clear</button>
            : <button className="btn-orange sm" onClick={() => set({ filter: DEFAULT_FILTER })}>Apply</button>}
        </div>
      </div>
    </Fade>
  );
}
