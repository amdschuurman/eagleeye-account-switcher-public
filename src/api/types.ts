// The Tauri command contract, docs/DESIGN.md section 1 ("Tauri command contract"). The engine (src-tauri) is the
// producer; this file is the only place the UI states the shapes. Keep both sides in step.

export type AccountStatus = "ready" | "needs_login";

/** Phase 2: one category per account (DESIGN.md section 3). Default "smurf". */
export type Category = "main" | "smurf";

export interface Account {
  code: string; // 8-hex Battle.net account code, the id
  label: string;
  battletag?: string | null; // absent when unknown (engine skips None)
  emailMasked: string;
  order: number;
  lastUsed?: number | null; // unix time, absent until first use
  addedAt?: number;
  status: AccountStatus;
  category?: Category; // absent on an engine that predates phase 2; read as "smurf"
}

export interface SwitchResult {
  ok: boolean;
  status: "done" | "needs_login" | "error";
  ms: number;
  message: string;
}

export interface ImportSummary {
  imported: number;
  needsLogin: number;
  skipped: number;
}

export interface Settings {
  battlenetPath?: string | null; // absent = found automatically (registry, then the default folder)
  closeAfterSwitch: boolean;
  minimizeToTray: boolean;
  overfastUrl?: string; // phase 2, default https://overfast-api.tekrop.fr
  discordPresence: boolean; // show the app in Discord while its window is open (default true)
}

// ---- Phase 2 ranks (DESIGN.md "Phase 2 command contract") ----
// "emerald" is not in the written contract but the live career page and OverFast both report it (between Platinum and
// Diamond, seen 2026-10-23), so the UI accepts it. OverFast calls Champion "ultimate"; the engine maps that to "champion".
export type Tier = "bronze" | "silver" | "gold" | "platinum" | "emerald" | "diamond" | "master" | "grandmaster" | "champion";
export type Role = "tank" | "damage" | "support" | "open";
export type Division = 1 | 2 | 3 | 4 | 5;

export interface RankValue {
  tier: Tier;
  division: Division;
}

export interface RoleRank extends RankValue {
  source: "profile" | "manual";
  placement: boolean;
  predicted?: RankValue | null; // only when placement is true
}

/** What the last lookup said: "ok" (also before the first one), a private profile, a BattleTag OverFast does not
 *  know yet, or a lookup that failed (text in `error`). */
export type RankStatus = "ok" | "private" | "notFound" | "error";

export interface RankInfo {
  battletag: string;
  season: number | null;
  updatedAt: number | null; // ms, last successful lookup
  checkedAt: number | null; // ms, last attempt, successful or not
  status: RankStatus;
  recheckAt: number | null; // ms, for "notFound": when OverFast looks for the player again
  privateProfile: boolean;
  error?: string | null;
  roles: Record<Role, RoleRank | null>;
  avatarFile?: string | null; // the Overwatch player icon, a file in icon_dir()'s "avatars" folder, only while it is on disk
}

export type SwitchStage = "closing_game" | "closing" | "restoring" | "launching" | "verifying" | "done" | "needs_login" | "error";

export interface SwitchProgress {
  code: string;
  stage: SwitchStage;
  ms: number;
  game?: string; // only on "closing_game": the game being closed, e.g. "Overwatch"
}

export interface AddProgress {
  stage: "waiting_login" | "captured" | "error";
  message?: string;
}

export interface ImportProgress {
  done: number;
  total: number;
  label: string;
  status: string; // "start" while that account is being signed in, then "imported" | "needs_login"
}

// ---- Optional updates (updater.rs): checked on a schedule, offered in the UI, installed only on "Update now" ----
// "rateLimited": a manual check inside the cooldown (60 s after every check, longer after a server 429 / Retry-After) with
// no update known; the engine made no network call and says when the next check is allowed (retryAt).
// "disabled": this build has no update endpoint or key, so it never checks (Settings says so).
export type UpdateStatus = "idle" | "checking" | "available" | "downloading" | "installing" | "error" | "upToDate" | "rateLimited" | "disabled";

export interface UpdateState {
  status: UpdateStatus;
  current: string; // the running version
  version?: string; // the offered version; also kept on an error after a failed install, so Retry installs again
  notes?: string;
  progress?: { downloaded: number; total?: number }; // bytes, while downloading
  error?: string; // plain text, on "error"
  checkedAt?: number; // ms epoch of the last completed check
  retryAt?: number; // ms epoch: the earliest next manual check; the UI keeps its button off until then
}

export interface EventMap {
  "switch-progress": SwitchProgress;
  "add-progress": AddProgress;
  "import-progress": ImportProgress;
  "ranks-updated": { code: string };
  "ranks-error": { code: string | null; error: string }; // the refresh itself could not run (null = all accounts)
  "update-state": UpdateState;
}

export interface Api {
  listAccounts(): Promise<Account[]>;
  currentAccount(): Promise<string | null>;
  switchAccount(code: string): Promise<SwitchResult>;
  addAccount(): Promise<Account>;
  cancelAdd(): Promise<void>;
  saveCurrent(): Promise<Account>;
  importTcno(): Promise<ImportSummary>;
  tcnoAvailable(): Promise<number>;
  renameAccount(code: string, label: string): Promise<void>;
  removeAccount(code: string): Promise<void>;
  reorderAccounts(codes: string[]): Promise<void>;
  getSettings(): Promise<Settings>;
  setSettings(settings: Settings): Promise<void>;
  getRanks(code: string): Promise<RankInfo | null>;
  getAllRanks(): Promise<Record<string, RankInfo>>;
  refreshRanks(code: string | null, force?: boolean): Promise<void>; // force skips the engine's 1 h cache
  setRankOverride(code: string, role: Role, rank: RoleRank | null): Promise<void>;
  setCategory(code: string, category: Category): Promise<void>;
  iconDir(): Promise<string>;
  updateState(): Promise<UpdateState>;
  /** Check now; records the answer only, installs nothing. Inside the cooldown it makes no network call and answers
   *  "available" when an update is known, else "rateLimited" with retryAt. */
  checkUpdate(): Promise<UpdateState>;
  /** "Update now": download, wait for a running switch, install, restart. Resolves at once; follow "update-state". */
  installUpdate(): Promise<void>;
  /** Turns a file in icon_dir() into a URL the webview can load (convertFileSrc in Tauri, a dev folder in the mock). */
  iconUrl(dir: string, file: string): string;
  /** Same for a player icon (RankInfo.avatarFile), which lives in icon_dir()'s "avatars" folder. */
  avatarUrl(dir: string, file: string): string;
  on<K extends keyof EventMap>(event: K, cb: (payload: EventMap[K]) => void): Promise<() => void>;
}
