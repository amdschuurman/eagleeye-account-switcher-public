// Rank card shown after hovering an account for 250 ms: compact dark tooltip beside the card, four rows, icons left.
import { useLayoutEffect, useRef, useState } from "react";
import type { Account, RankInfo, RankValue, Role } from "../api";
import { useApp, type HoverAt } from "../store";
import { ago, checkedText, isStale, newestSeason, ROLE_NAME, ROLES, roleText, seasonText, statusText } from "../rank";
import { BlankEmblem, RoleIcon, TierIcon } from "./RankEmblem";
import { Icon } from "./Icon";

export const HOVER_DELAY_MS = 250;

interface Row {
  role: Role;
  kind: "rank" | "predicted" | "placement" | "unranked" | "nodata";
  rank: RankValue | null;
  manual: boolean;
  text: string;
}

function rows(info: RankInfo | undefined): Row[] {
  return ROLES.map((role) => {
    const r = info?.roles[role] ?? null;
    const text = roleText(info, role);
    if (!info) return { role, kind: "nodata", rank: null, manual: false, text };
    if (!r) return { role, kind: "unranked", rank: null, manual: false, text };
    if (r.placement) return { role, kind: r.predicted ? "predicted" : "placement", rank: r.predicted ?? null, manual: r.source === "manual", text };
    return { role, kind: "rank", rank: r, manual: r.source === "manual", text };
  });
}

/** Small words under or beside a rank: where it came from, or that it is a guess. */
function Tag({ row }: { row: Row }) {
  if (row.kind === "predicted" || row.kind === "placement") return <span className="rk-tag predicted">Predicted</span>;
  if (row.manual) return <span className="rk-tag">Manual</span>;
  return null;
}

function Private() {
  return (
    <div className="rk-private">
      <span className="rk-private-ic"><Icon name="lock" size={16} /></span>
      <span>
        <b>Profile is private</b>
        <span className="rk-private-sub">Set the Overwatch career profile to public to see ranks here.</span>
      </span>
    </div>
  );
}

/** The engine could not read the profile (OverFast does not know the BattleTag yet, network), and has nothing older to show. */
function Problem({ text }: { text: string }) {
  return (
    <div className="rk-private">
      <span className="rk-private-ic"><Icon name="medal" size={16} /></span>
      <span><b>No ranks</b><span className="rk-private-sub">{text}</span></span>
    </div>
  );
}

/** Private, not found or an error with nothing to show, or null when the rows can be drawn. */
function notice(info: RankInfo | undefined) {
  if (info?.privateProfile) return <Private />;
  if (info && (info.status === "notFound" || info.status === "error") && info.season == null && ROLES.every((r) => !info.roles[r])) {
    return <Problem text={statusText(info)} />;
  }
  return null;
}

/** "updated 5 min ago, checked 14:02": the last good data and the last attempt, which differ after a failure or a
 *  not-found answer. */
function when(info: RankInfo | undefined): string {
  if (!info?.checkedAt) return ago(info?.updatedAt);
  const last = info.updatedAt ? ago(info.updatedAt) : "";
  const updated = info.error ? (last ? `Could not update, ${last.replace("updated ", "last ")}` : "Could not update") : last;
  return [updated, checkedText(info)].filter(Boolean).join(", ");
}

function Footer({ info, newest }: { info: RankInfo | undefined; newest: number | null }) {
  return (
    <div className="rk-foot">
      <span className={isStale(info, newest) ? "stale" : ""}>{info?.privateProfile ? "Season unknown" : seasonText(info, newest)}</span>
      <span title={info?.error || statusText(info) || undefined}>{when(info)}</span>
    </div>
  );
}

function Head({ account }: { account: Account }) {
  return (
    <div className="rk-head">
      <span className="rk-name">{account.label}</span>
      {account.battletag && <span className="rk-tagline">{account.battletag}</span>}
    </div>
  );
}

// ---- A: compact tooltip ----
function VariantA({ account, info, newest }: Props) {
  return (
    <>
      <Head account={account} />
      {notice(info) ?? (
        <div className="a-rows">
          {rows(info).map((r) => (
            <div key={r.role} className={"a-row k-" + r.kind}>
              <RoleIcon role={r.role} size={15} />
              <span className="a-crest">{r.rank ? <span className={r.kind === "predicted" ? "is-predicted" : ""}><TierIcon tier={r.rank.tier} size={26} /></span> : <BlankEmblem size={22} />}</span>
              <span className="a-text">
                <span className="a-role">{ROLE_NAME[r.role]}<Tag row={r} /></span>
                <span className="a-rank">{r.text}</span>
              </span>
            </div>
          ))}
        </div>
      )}
      <Footer info={info} newest={newest} />
    </>
  );
}

interface Props { account: Account; info: RankInfo | undefined; newest: number | null }

const GAP = 10;

function place(at: HoverAt, w: number, h: number) {
  const W = window.innerWidth, H = window.innerHeight;
  const clampY = (y: number) => Math.max(60, Math.min(y, H - h - 8));
  const right = at.right + GAP + w <= W - 8;
  return { x: right ? at.right + GAP : at.left - GAP - w, y: clampY(at.top), side: right ? "right" : "left" };
}

export function HoverCard() {
  const hover = useApp((s) => s.hover);
  const blocked = useApp((s) => !!s.menu || !!s.editRanks || !!s.confirmRemove || !!s.adding || !!s.renaming || s.filterOpen || s.sortOpen);
  const account = useApp((s) => s.accounts.find((a) => a.code === hover?.code));
  const info = useApp((s) => (hover ? s.ranks[hover.code] : undefined));
  const newest = useApp((s) => newestSeason(s.ranks));
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ x: number; y: number; side: string } | null>(null);

  useLayoutEffect(() => {
    if (!hover || !ref.current) { setPos(null); return; }
    const r = ref.current.getBoundingClientRect();
    setPos(place(hover, r.width, r.height));
  }, [hover, info, account]);

  if (!hover || !account || blocked) return null;
  return (
    <div className="rank-layer" aria-hidden="false">
      <div ref={ref} key={hover.code} role="tooltip" aria-label={`${account.label} ranks`}
        className={`rank-card rank-a side-${pos?.side ?? "right"}` + (isStale(info, newest) ? " stale" : "") + (pos ? " shown" : "")}
        style={{ left: pos?.x ?? -9999, top: pos?.y ?? 0, ["--card-h" as string]: `${hover.bottom - hover.top}px` }}>
        <VariantA account={account} info={info} newest={newest} />
      </div>
    </div>
  );
}
