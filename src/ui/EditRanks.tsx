// Right-click > Edit ranks: a manual value per role (tier + division), or "In placements" with an optional predicted
// rank. "From profile" clears the manual value (set_rank_override with null). Manual values win until a newer season
// arrives from the profile (the engine's rule, DESIGN.md section 3).
import { useEffect, useRef, useState } from "react";
import type { Division, RankInfo, RankValue, Role, RoleRank, Tier } from "../api";
import { useApp } from "../store";
import { DIVISIONS, rankName, ROLE_NAME, ROLES, TIERS, TIER_NAME } from "../rank";
import { Fade } from "./Fade";
import { Icon } from "./Icon";
import { RoleIcon, TierIcon } from "./RankEmblem";

interface Draft {
  mode: "profile" | "manual";
  tier: Tier;
  division: Division;
  placement: boolean;
  predicted: RankValue | null;
}

function draftOf(r: RoleRank | null | undefined): Draft {
  if (!r || r.source !== "manual") return { mode: "profile", tier: r?.tier ?? "gold", division: r?.division ?? 3, placement: !!r?.placement, predicted: r?.predicted ?? null };
  return { mode: "manual", tier: r.tier, division: r.division, placement: r.placement, predicted: r.predicted ?? null };
}

/** The value sent for a role: null = back to the profile. A placement keeps a tier/division the contract requires;
 *  it is the prediction when there is one (never shown as a settled rank, placement wins). */
function valueOf(d: Draft): RoleRank | null {
  if (d.placement) {
    const base = d.predicted ?? { tier: d.tier, division: d.division };
    return { tier: base.tier, division: base.division, source: "manual", placement: true, predicted: d.predicted };
  }
  if (d.mode === "profile") return null;
  return { tier: d.tier, division: d.division, source: "manual", placement: false };
}

function same(a: RoleRank | null, r: RoleRank | null | undefined) {
  const b = r && r.source === "manual" ? r : null;
  return JSON.stringify(a) === JSON.stringify(b ? { tier: b.tier, division: b.division, source: "manual", placement: b.placement, ...(b.placement ? { predicted: b.predicted ?? null } : {}) } : null);
}

function TierSelect({ value, onChange, label, allowProfile, profileHint }: {
  value: Tier | "profile" | "none"; onChange: (v: Tier | "profile" | "none") => void; label: string; allowProfile?: boolean; profileHint?: string;
}) {
  return (
    <span className="select-wrap">
      {value !== "profile" && value !== "none" ? <TierIcon tier={value} size={18} /> : <span className="select-ic-blank" />}
      <select className="select" value={value} aria-label={label} onChange={(e) => onChange(e.target.value as Tier | "profile" | "none")}>
        {allowProfile && <option value="profile">{profileHint ? `Profile, ${profileHint}` : "From profile"}</option>}
        {!allowProfile && <option value="none">No prediction</option>}
        {[...TIERS].reverse().map((t) => <option key={t} value={t}>{allowProfile ? TIER_NAME[t] : `Predicted ${TIER_NAME[t]}`}</option>)}
      </select>
      <Icon name="chevronDown" size={12} />
    </span>
  );
}

function DivSelect({ value, onChange, disabled, label }: { value: Division; onChange: (v: Division) => void; disabled?: boolean; label: string }) {
  return (
    <span className={"select-wrap div" + (disabled ? " disabled" : "")}>
      <select className="select" value={value} disabled={disabled} aria-label={label} onChange={(e) => onChange(Number(e.target.value) as Division)}>
        {DIVISIONS.map((d) => <option key={d} value={d}>{d}</option>)}
      </select>
      <Icon name="chevronDown" size={12} />
    </span>
  );
}

function RoleRow({ role, draft, profile, onChange }: { role: Role; draft: Draft; profile: string; onChange: (d: Draft) => void }) {
  const p = (x: Partial<Draft>) => onChange({ ...draft, ...x });
  return (
    <div className={"er-row" + (draft.placement ? " placing" : "")}>
      <span className="er-role"><RoleIcon role={role} size={16} />{ROLE_NAME[role]}</span>
      <span className="er-rank">
        {draft.placement ? (
          <>
            <TierSelect label={`${ROLE_NAME[role]} predicted tier`} value={draft.predicted?.tier ?? "none"}
              onChange={(v) => p({ predicted: v === "none" || v === "profile" ? null : { tier: v, division: draft.predicted?.division ?? 3 } })} />
            <DivSelect label={`${ROLE_NAME[role]} predicted division`} value={draft.predicted?.division ?? 3} disabled={!draft.predicted}
              onChange={(v) => draft.predicted && p({ predicted: { ...draft.predicted, division: v } })} />
          </>
        ) : (
          <>
            <TierSelect label={`${ROLE_NAME[role]} tier`} allowProfile profileHint={profile} value={draft.mode === "profile" ? "profile" : draft.tier}
              onChange={(v) => (v === "profile" || v === "none" ? p({ mode: "profile" }) : p({ mode: "manual", tier: v }))} />
            <DivSelect label={`${ROLE_NAME[role]} division`} value={draft.division} disabled={draft.mode === "profile"} onChange={(v) => p({ division: v })} />
          </>
        )}
      </span>
      <label className="check er-place">
        <input type="checkbox" checked={draft.placement} onChange={(e) => p({ placement: e.target.checked, mode: e.target.checked ? "manual" : draft.mode })} />
        <span className="box"><Icon name="check" size={12} stroke={2.6} /></span>
        <span>In placements</span>
      </label>
    </div>
  );
}

function profileHint(info: RankInfo | undefined, role: Role): string {
  if (!info) return "no data";
  if (info.privateProfile) return "private";
  const r = info.roles[role];
  if (!r || r.source === "manual") return r ? "manual now" : "unranked";
  return r.placement ? "placements" : rankName(r);
}

export function EditRanks() {
  const code = useApp((s) => s.editRanks);
  const shownCode = useRef<string | null>(null);
  if (code) shownCode.current = code;
  const acct = useApp((s) => s.accounts.find((a) => a.code === shownCode.current));
  const info = useApp((s) => (shownCode.current ? s.ranks[shownCode.current] : undefined));
  const { set, saveRanks } = useApp.getState();
  const [drafts, setDrafts] = useState<Record<Role, Draft> | null>(null);
  const saveBtn = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!code) return;
    const info0 = useApp.getState().ranks[code];
    setDrafts(Object.fromEntries(ROLES.map((r) => [r, draftOf(info0?.roles[r])])) as Record<Role, Draft>);
    saveBtn.current?.focus();
    const key = (e: KeyboardEvent) => { if (e.key === "Escape") set({ editRanks: null }); };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, [code, set]);

  const save = () => {
    if (!drafts || !shownCode.current) return;
    const changes: Partial<Record<Role, RoleRank | null>> = {};
    for (const r of ROLES) {
      const v = valueOf(drafts[r]);
      if (!same(v, info?.roles[r])) changes[r] = v;
    }
    void saveRanks(shownCode.current, changes);
  };

  return (
    <Fade show={!!code} className="layer dim">
      <div className="dialog edit-ranks" role="dialog" aria-modal="true" aria-labelledby="er-title">
        <h2 id="er-title">Ranks for {acct?.label ?? "this account"}</h2>
        <p className="er-intro">Set a rank by hand when the profile is private or behind. A manual rank stays until a new season shows up on the profile.</p>
        <div className="er-rows">
          {drafts && ROLES.map((r) => <RoleRow key={r} role={r} draft={drafts[r]} profile={profileHint(info, r)} onChange={(d) => setDrafts({ ...drafts, [r]: d })} />)}
        </div>
        <div className="dialog-foot">
          <button className="btn-grey" onClick={() => set({ editRanks: null })}>Cancel</button>
          <button ref={saveBtn} className="btn-orange" onClick={save}>Save</button>
        </div>
      </div>
    </Fade>
  );
}
