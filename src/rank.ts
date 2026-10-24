// Rank arithmetic and wording, shared by the hover card, the card badge, sorting and filtering.
import type { Division, RankInfo, RankValue, Role, RoleRank, Tier } from "./api";

export const TIERS: Tier[] = ["bronze", "silver", "gold", "platinum", "emerald", "diamond", "master", "grandmaster", "champion"];
export const ROLES: Role[] = ["tank", "damage", "support", "open"];
export const DIVISIONS: Division[] = [1, 2, 3, 4, 5];

export const ROLE_NAME: Record<Role, string> = { tank: "Tank", damage: "Damage", support: "Support", open: "Open Queue" };
export const TIER_NAME: Record<Tier, string> = {
  bronze: "Bronze", silver: "Silver", gold: "Gold", platinum: "Platinum", emerald: "Emerald",
  diamond: "Diamond", master: "Master", grandmaster: "Grandmaster", champion: "Champion",
};

/** Higher is better: Champion 1 is the top, Bronze 5 the bottom. Unranked is -1. */
export function score(r: RankValue | null | undefined): number {
  if (!r) return -1;
  return TIERS.indexOf(r.tier) * 5 + (5 - r.division);
}

export function fromScore(s: number): RankValue {
  return { tier: TIERS[Math.floor(s / 5)], division: (5 - (s % 5)) as Division };
}

export const MAX_SCORE = score({ tier: "champion", division: 1 });

export function rankName(r: RankValue): string {
  return `${TIER_NAME[r.tier]} ${r.division}`;
}

/** The rank that counts for sorting and filtering, the engine's RoleRank::effective: the prediction while in
 *  placements (no prediction = unranked), else the rank itself. */
export function effective(r: RoleRank | null | undefined): RankValue | null {
  if (!r) return null;
  if (r.placement) return r.predicted ?? null;
  return { tier: r.tier, division: r.division };
}

/** The best role by effective rank; predicted = that rank is a placement guess (drawn faded). */
export function bestRole(info: RankInfo | null | undefined): { role: Role; rank: RankValue; predicted: boolean } | null {
  if (!info || info.privateProfile) return null;
  let best: { role: Role; rank: RankValue; predicted: boolean } | null = null;
  for (const role of ROLES) {
    const r = effective(info.roles[role]);
    if (r && (!best || score(r) > score(best.rank))) best = { role, rank: r, predicted: !!info.roles[role]?.placement };
  }
  return best;
}

/** The text a role row shows, written in full. */
export function roleText(info: RankInfo | null | undefined, role: Role): string {
  if (!info) return "No data yet";
  if (info.privateProfile) return "Profile is private";
  const r = info.roles[role];
  if (!r) return "Unranked";
  if (r.placement) return r.predicted ? `In placements, predicted ${rankName(r.predicted)}` : "In placements";
  return rankName(r);
}

export function ago(ms: number | null | undefined, now = Date.now()): string {
  if (!ms) return "not checked yet";
  const s = Math.max(0, Math.round((now - ms) / 1000));
  if (s < 60) return "updated just now";
  const m = Math.round(s / 60);
  if (m < 60) return `updated ${m} min ago`;
  const h = Math.round(m / 60);
  if (h < 48) return `updated ${h} h ago`;
  return `updated ${Math.round(h / 24)} days ago`;
}

/** A clock time as HH:MM (24 h, local). */
export function clock(ms: number): string {
  const d = new Date(ms);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

/** Why there are no profile ranks, in words; "" when the last lookup was fine. OverFast caches an unknown BattleTag
 *  for a while, so a profile made public minutes ago can still be "not found" until its recheck time. */
export function statusText(info: RankInfo | null | undefined, now = Date.now()): string {
  if (!info) return "";
  switch (info.status) {
    case "private": return "Profile private";
    case "notFound": return info.recheckAt && info.recheckAt > now ? `Not found yet, OverFast checks again at ${clock(info.recheckAt)}` : "Not found by OverFast";
    case "error": return info.error ?? "Could not update";
    default: return "";
  }
}

/** When the engine last tried, successful or not. */
export function checkedText(info: RankInfo | null | undefined): string {
  return info?.checkedAt ? `checked ${clock(info.checkedAt)}` : "not checked yet";
}

export function newestSeason(all: Record<string, RankInfo>): number | null {
  let n: number | null = null;
  for (const r of Object.values(all)) if (r.season != null && (n == null || r.season > n)) n = r.season;
  return n;
}

export function isStale(info: RankInfo | null | undefined, newest: number | null): boolean {
  return !!info && info.season != null && newest != null && info.season < newest;
}

export function seasonText(info: RankInfo | null | undefined, newest: number | null): string {
  if (!info || info.season == null) return "Season unknown";
  return isStale(info, newest) ? `Season ${info.season}, last season` : `Season ${info.season}`;
}
