// Blizzard's own rank art, read from the engine's icon folder (icon_dir: tier_<tier>.png, div_<n>.png, role_<role>.svg).
// The art is never bundled: a missing file, or no icon folder yet, falls back to a neutral drawn placeholder.
import { useState } from "react";
import { api, type RankValue, type Role, type Tier } from "../api";
import { useApp } from "../store";
import { ROLE_NAME, TIER_NAME } from "../rank";

let iconUrlFn: ((dir: string, file: string) => string) | null = null;
let avatarUrlFn: ((dir: string, file: string) => string) | null = null;
void api().then((a) => { iconUrlFn = a.iconUrl; avatarUrlFn = a.avatarUrl; });

function useIconUrl(file: string): string | null {
  const dir = useApp((s) => s.iconDir);
  if (!dir || !iconUrlFn) return null;
  return iconUrlFn(dir, file);
}

/** The account's Overwatch player icon (RankInfo.avatarFile in the icon folder's "avatars"), or null when there is none. */
export function useAvatarUrl(code: string): string | null {
  const dir = useApp((s) => s.iconDir);
  const file = useApp((s) => s.ranks[code]?.avatarFile ?? null);
  if (!dir || !file || !avatarUrlFn) return null;
  return avatarUrlFn(dir, file);
}

/** Tier colours, only for the faint glow behind a big emblem and the badge's text tint. */
export const TIER_HUE: Record<Tier, string> = {
  bronze: "199, 119, 62", silver: "190, 200, 212", gold: "232, 179, 58", platinum: "150, 214, 226", emerald: "63, 214, 160",
  diamond: "127, 170, 255", master: "150, 220, 180", grandmaster: "170, 150, 255", champion: "222, 150, 255",
};

/** An image that turns into the placeholder when its file is missing. */
function Art({ file, className, alt, fallback }: { file: string; className: string; alt: string; fallback: JSX.Element }) {
  const url = useIconUrl(file);
  const [broken, setBroken] = useState<string | null>(null);
  if (!url || broken === url) return fallback;
  return <img className={className} src={url} alt={alt} draggable={false} onError={() => setBroken(url)} />;
}

/** Neutral hexagon: unranked, private, or art not fetched yet. */
export function BlankEmblem({ size, label }: { size: number; label?: string }) {
  return (
    <svg className="emblem-blank" width={size} height={size} viewBox="0 0 40 40" aria-hidden={!label} aria-label={label}>
      <path d="M20 4l13.5 7.8v15.6L20 35.2 6.5 27.4V11.8z" style={{ fill: "var(--w-04)", stroke: "var(--w-22)" }} strokeWidth="1.5" strokeLinejoin="round" />
      <path d="M14.5 20h11" style={{ stroke: "var(--w-25)" }} strokeWidth="1.5" strokeLinecap="round" />
    </svg>
  );
}

export function TierIcon({ tier, size }: { tier: Tier; size: number }) {
  return (
    <span className="tier-icon" style={{ height: size, width: size * 1.2 }}>
      <Art file={`tier_${tier}.png`} className="tier-img" alt={TIER_NAME[tier]} fallback={<BlankEmblem size={size} />} />
    </span>
  );
}

/** Tier crest over its division wings, the way the career page draws a rank. size = crest height. */
export function Emblem({ rank, size }: { rank: RankValue; size: number }) {
  return (
    <span className="emblem" style={{ ["--s" as string]: `${size}px` }} title={`${TIER_NAME[rank.tier]} ${rank.division}`}>
      <Art file={`div_${rank.division}.png`} className="emblem-div" alt="" fallback={<span className="emblem-div-blank">{rank.division}</span>} />
      <Art file={`tier_${rank.tier}.png`} className="emblem-tier" alt={TIER_NAME[rank.tier]} fallback={<span className="emblem-tier-blank"><BlankEmblem size={size} /></span>} />
    </span>
  );
}

const ROLE_FALLBACK: Record<Role, string> = {
  tank: "M20 3c-5 0-9 1-12 2v12c0 7 5 13 12 19 7-6 12-12 12-19V5c-3-1-7-2-12-2z",
  damage: "M14 6h4v24h-4zM22 6h4v24h-4zM12 32h8v3h-8zM20 32h8v3h-8z",
  support: "M16 5h8v11h11v8H24v11h-8V24H5v-8h11z",
  open: "M20 4l14 8v16l-14 8-14-8V12z",
};

/** Blizzard's role SVGs declare width="auto" on the root, so the img always gets an explicit box (.role-img, 100%). */
export function RoleIcon({ role, size = 16 }: { role: Role; size?: number }) {
  return (
    <span className="role-icon" style={{ width: size, height: size }}>
      <Art file={`role_${role}.svg`} className="role-img" alt={ROLE_NAME[role]}
        fallback={<svg width={size} height={size} viewBox="0 0 40 40" aria-label={ROLE_NAME[role]}><path d={ROLE_FALLBACK[role]} fill="currentColor" /></svg>} />
    </span>
  );
}
