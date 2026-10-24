// Mock engine for a plain browser (no Tauri): fake accounts only, simulated timings, same events as the real engine.
// Query flags for screenshots and dev: ?empty (no saved accounts, TcNo has 10), ?slow (timings x3), ?noavatars (letters only).
// Updates: ?update (= available), ?update=downloading|installing|error|checkerror|uptodate|ratelimited|disabled starts in that
// state and holds still there (screenshots); without the flag "Check for updates" answers "up to date". Like the engine,
// every check starts a 60 s cooldown; a check inside it makes no "network call" and answers available or rateLimited.
// Rank icons in the mock come from temp/rank-icons (Blizzard art, fetched for local dev by `npm run icons`, never committed
// or bundled): vite dev serves that folder by path, the screenshot test serves it by route. Missing file = placeholder.
import type { Account, Api, Category, EventMap, RankInfo, RoleRank, Settings, Tier, UpdateState } from "./types";

const q = new URLSearchParams(location.search);
const slow = q.has("slow") ? 3 : 1;
const wait = (ms: number) => new Promise((r) => setTimeout(r, ms * slow));

// Invented players (no real people): the label is the BattleTag's name, as an account saved from Battle.net starts out.
const FAKE: Array<[string, string, string, Account["status"], Category]> = [
  ["Kestrel", "Kestrel#2187", "ke***@g***.com", "ready", "main"],
  ["Nimbus", "Nimbus#1934", "ni***@o***.com", "ready", "smurf"],
  ["Halcyon", "Halcyon#2741", "ha***@g***.com", "ready", "smurf"],
  ["Vantage", "Vantage#1188", "va***@p***.me", "ready", "main"],
  ["Pebble", "Pebble#2290", "pe***@g***.com", "needs_login", "main"],
  ["Lumen", "Lumen#1462", "lu***@o***.com", "ready", "smurf"],
  ["Corvid", "Corvid#2015", "co***@y***.com", "ready", "main"],
  ["Tessel", "Tessel#3307", "te***@g***.com", "ready", "smurf"],
];

const SEASON = 22;
const rr = (tier: Tier, division: RoleRank["division"], source: RoleRank["source"] = "profile"): RoleRank => ({ tier, division, source, placement: false });
const MIN = 60_000;
// One per fake account, same order as FAKE: a Champion, a Grandmaster, Platinum and Gold mixes, a role in placements,
// a fully unranked profile, a private profile, a stale season and a manual value.
const FAKE_PROFILES: Array<(now: number) => Omit<RankInfo, "battletag" | "checkedAt" | "status" | "recheckAt">> = [
  (n) => ({ season: SEASON, updatedAt: n - 4 * MIN, privateProfile: false, roles: { tank: rr("grandmaster", 2), damage: rr("champion", 3), support: rr("master", 1), open: rr("grandmaster", 5) } }),
  (n) => ({ season: SEASON, updatedAt: n - 23 * MIN, privateProfile: false, roles: { tank: rr("master", 3), damage: rr("diamond", 2), support: rr("grandmaster", 1), open: null } }),
  (n) => ({ season: SEASON, updatedAt: n - 12 * MIN, privateProfile: false, roles: { tank: rr("platinum", 2), damage: rr("gold", 3), support: rr("platinum", 4), open: rr("gold", 1) } }),
  (n) => ({ season: SEASON, updatedAt: n - 38 * MIN, privateProfile: false, roles: { tank: rr("gold", 2), damage: { ...rr("bronze", 5, "manual"), placement: true, predicted: { tier: "diamond", division: 3 } }, support: rr("emerald", 4), open: rr("silver", 1) } }),
  (n) => ({ season: SEASON, updatedAt: n - 51 * MIN, privateProfile: false, roles: { tank: null, damage: null, support: null, open: null } }),
  (n) => ({ season: null, updatedAt: n - 7 * MIN, privateProfile: true, roles: { tank: null, damage: null, support: null, open: null } }),
  (n) => ({ season: SEASON - 2, updatedAt: n - 26 * 60 * MIN, privateProfile: false, roles: { tank: rr("diamond", 4), damage: rr("platinum", 1), support: rr("master", 5), open: null } }),
  (n) => ({ season: SEASON, updatedAt: n - 2 * MIN, privateProfile: false, roles: { tank: rr("silver", 5), damage: rr("bronze", 2), support: rr("gold", 4, "manual"), open: rr("emerald", 3) } }),
];

const FAKE_RANKS: Array<(now: number) => Omit<RankInfo, "battletag">> = FAKE_PROFILES.map((f) => (n) => {
  const p = f(n);
  return { ...p, checkedAt: p.updatedAt, status: p.privateProfile ? "private" : "ok", recheckAt: null };
});
// ?notfound: the unranked account becomes a BattleTag OverFast does not know yet (it looks again in 40 minutes).
if (q.has("notfound")) {
  FAKE_RANKS[4] = (n) => ({ season: null, updatedAt: null, checkedAt: n - 3 * MIN, status: "notFound", recheckAt: n + 40 * MIN, privateProfile: false, roles: { tank: null, damage: null, support: null, open: null } });
}

// Player icons: real Overwatch player icons on Blizzard's CDN, referenced by URL (never downloaded into the repo). The
// engine hands the UI a file name in its icon folder; the mock hands it the full URL and avatarUrl() passes it through.
const OW_ICON = "https://d15f34w2p8l1cc.cloudfront.net/overwatch/";
const MOCK_AVATARS: string[] = [
  "4cf4a15525c8f9a1d4ceb5c53d969d87d8c603b5ff5eae085d28a54aba64ec4d",
  "2c465ef38c2670682cdb917e3939eb23f8173b58c88814aa0a5d1571c9742605",
  "aaf2ccec2bf613fc8f0d511d86128777c9cc5e7565fadd4471011d676b9ebb04",
  "9e800299c75d50808dba04ec385fb17ac9e554d3b0ecc4bc4057dcfb6ec24c95",
  "76516371683932b558b1f58df3134e34f72b3d0c97077d8364cd73a7f313da92",
  "18e52d45a72727e34243f2f2fd18177c3cb737f9bdb0ad860503c42f21ad20d4",
  "b70c702d65bd952e46a80c1f041b4747c64f96041bcfdeef60b8a91a13816619",
  "73d5e84b0073517e11900748c83e7743c31dfd095dc112df88a7e04e5bc6949a",
].map((h) => `${OW_ICON}${h}.png`);

const MOCK_VERSION = "1.0.0";
const MOCK_NEXT = "1.1.0";
const MOCK_SIZE = 6_480_000;
const COOLDOWN_MS = 60_000; // the engine's minimum gap between two checks

function initialUpdate(): UpdateState {
  const base: UpdateState = { status: "idle", current: MOCK_VERSION };
  const offered = { version: MOCK_NEXT, notes: "Faster switching and a clearer rank card." };
  switch (q.has("update") ? q.get("update") || "available" : "") {
    case "available": return { ...base, status: "available", ...offered };
    case "downloading": return { ...base, status: "downloading", ...offered, progress: { downloaded: Math.round(MOCK_SIZE * 0.46), total: MOCK_SIZE } };
    case "installing": return { ...base, status: "installing", ...offered };
    case "error": return { ...base, status: "error", ...offered, error: "The download failed: the connection was reset." };
    case "checkerror": return { ...base, status: "error", error: "Could not check for updates: the update server did not answer." };
    case "uptodate": return { ...base, status: "upToDate" };
    case "ratelimited": { const t = Date.now(); return { ...base, status: "rateLimited", checkedAt: t - 15_000, retryAt: t + 45_000 }; }
    case "disabled": return { ...base, status: "disabled" };
    default: return base;
  }
}

const TCNO = ["Kestrel", "Nimbus", "Halcyon", "Vantage", "Pebble", "Lumen", "Corvid", "Tessel", "Marlow", "Quill"];

function fakeCode(i: number) {
  return (0x1a2b3c00 + i * 0x101).toString(16);
}

export function createMockApi(): Api {
  let accounts: Account[] = q.has("empty")
    ? []
    : FAKE.map(([label, battletag, emailMasked, status, category], i) => ({ code: fakeCode(i), label, battletag, emailMasked, order: i, status, category }));
  const now0 = Date.now();
  const ranks: Record<string, RankInfo> = {};
  accounts.forEach((a, i) => { ranks[a.code] = { battletag: a.battletag ?? "", ...FAKE_RANKS[i](now0), avatarFile: q.has("noavatars") ? null : MOCK_AVATARS[i % MOCK_AVATARS.length] }; });
  let current: string | null = q.has("empty") ? fakeCode(0) : accounts[0]?.code ?? null;
  let settings: Settings = { battlenetPath: "C:\\Program Files (x86)\\Battle.net", closeAfterSwitch: false, minimizeToTray: false };
  let switching = false;
  let addCancel: (() => void) | null = null;
  let tcno = q.has("empty") ? TCNO.length : 0;

  let update = initialUpdate();
  const frozen = q.has("update") && ["downloading", "installing"].includes(q.get("update") ?? "");

  const subs: { [K in keyof EventMap]?: Set<(p: EventMap[K]) => void> } = {};
  const emit = <K extends keyof EventMap>(ev: K, p: EventMap[K]) => (subs[ev] as Set<(p: EventMap[K]) => void> | undefined)?.forEach((cb) => cb(p));
  const sorted = () => [...accounts].sort((a, b) => a.order - b.order).map((a) => ({ ...a }));
  const find = (code: string) => {
    const a = accounts.find((x) => x.code === code);
    if (!a) throw "That account is no longer saved.";
    return a;
  };

  return {
    async listAccounts() { await wait(60); return sorted(); },
    async currentAccount() { return current; },
    async switchAccount(code) {
      const acct = find(code);
      if (switching) return { ok: false, status: "error", ms: 0, message: "A switch is already running." };
      if (current === code) return { ok: true, status: "done", ms: 120, message: "" };
      switching = true;
      const t0 = performance.now();
      const step = async (stage: EventMap["switch-progress"]["stage"], ms: number) => {
        emit("switch-progress", { code, stage, ms: Math.round(performance.now() - t0) });
        await wait(ms);
      };
      try {
        await step("closing", 700);
        await step("restoring", 350);
        await step("launching", 1300);
        if (acct.status === "needs_login") {
          await step("verifying", 900);
          current = null;
          const ms = Math.round(performance.now() - t0);
          emit("switch-progress", { code, stage: "needs_login", ms });
          return { ok: false, status: "needs_login", ms, message: "Battle.net needs this account's password once." };
        }
        await step("verifying", 2200);
        current = code;
        acct.lastUsed = Date.now();
        const ms = Math.round(performance.now() - t0);
        emit("switch-progress", { code, stage: "done", ms });
        return { ok: true, status: "done", ms, message: "" };
      } finally {
        switching = false;
      }
    },
    addAccount() {
      return new Promise<Account>((resolve, reject) => {
        emit("add-progress", { stage: "waiting_login" });
        const t = setTimeout(() => {
          const i = accounts.length + 20;
          const a: Account = { code: fakeCode(i), label: `Account ${accounts.length + 1}`, battletag: "Wren#2468", emailMasked: "wr***@g***.com", order: accounts.length, lastUsed: Date.now(), status: "ready" };
          accounts.push(a);
          current = a.code;
          emit("add-progress", { stage: "captured" });
          addCancel = null;
          resolve({ ...a });
        }, 9000 * slow);
        addCancel = () => { clearTimeout(t); addCancel = null; reject("Cancelled."); };
      });
    },
    async cancelAdd() { addCancel?.(); },
    async saveCurrent() {
      await wait(1800);
      if (!current) throw "Battle.net is not signed in.";
      const existing = accounts.find((a) => a.code === current);
      if (existing) return { ...existing };
      const a: Account = { code: current, label: "Kestrel", battletag: "Kestrel#2187", emailMasked: "ke***@g***.com", order: accounts.length, lastUsed: Date.now(), status: "ready" };
      accounts.push(a);
      return { ...a };
    },
    async importTcno() {
      const total = tcno;
      let imported = 0, needsLogin = 0;
      for (let i = 0; i < total; i++) {
        emit("import-progress", { done: i, total, label: TCNO[i] ?? "", status: "start" });
        await wait(650);
        const status = i % 4 === 3 ? "needs_login" : "ready";
        if (status === "ready") imported++; else needsLogin++;
        const [label, battletag, emailMasked] = FAKE[i % FAKE.length];
        accounts.push({ code: fakeCode(i), label: TCNO[i] ?? label, battletag, emailMasked, order: i, status, category: "smurf" });
        emit("import-progress", { done: i + 1, total, label: TCNO[i] ?? label, status: status === "ready" ? "imported" : "needs_login" });
      }
      tcno = 0;
      return { imported, needsLogin, skipped: 0 };
    },
    async tcnoAvailable() { return tcno; },
    async renameAccount(code, label) { find(code).label = label; },
    async removeAccount(code) { accounts = accounts.filter((a) => a.code !== code); },
    async reorderAccounts(codes) { codes.forEach((c, i) => { const a = accounts.find((x) => x.code === c); if (a) a.order = i; }); },
    async getSettings() { return { ...settings }; },
    async setSettings(s) { settings = { ...s }; },
    async getRanks(code) { await wait(30); return ranks[code] ? structuredClone(ranks[code]) : null; },
    async getAllRanks() { await wait(60); return structuredClone(ranks); },
    async refreshRanks(code, force) {
      if (!force) return; // the mock's fake data is always "fresh" for the 1 h cache
      const codes = code ? [code] : accounts.map((a) => a.code);
      await wait(400);
      if (q.has("rankerror")) { emit("ranks-error", { code, error: "Could not refresh ranks: could not read ranks.json" }); return; }
      for (const c of codes) if (ranks[c]) { const t = Date.now(); ranks[c].checkedAt = t; if (ranks[c].status !== "notFound") ranks[c].updatedAt = t; emit("ranks-updated", { code: c }); }
    },
    async setRankOverride(code, role, rank) {
      find(code);
      const info = (ranks[code] ??= { battletag: find(code).battletag ?? "", season: SEASON, updatedAt: null, checkedAt: null, status: "ok", recheckAt: null, privateProfile: false, roles: { tank: null, damage: null, support: null, open: null } });
      info.roles[role] = rank ? { ...rank, source: "manual" } : null; // the mock has no profile value to fall back to
      emit("ranks-updated", { code });
    },
    async setCategory(code, category) { find(code).category = category; },
    async iconDir() { return "/temp/rank-icons"; },
    iconUrl(dir, file) { return `${dir}/${file}`; },
    avatarUrl(_dir, file) { return file; }, // already a CDN URL in the mock (see MOCK_AVATARS)
    async updateState() { return { ...update }; },
    async checkUpdate() {
      if (["checking", "downloading", "installing", "disabled"].includes(update.status)) return { ...update };
      const setU = (u: UpdateState) => { update = u; emit("update-state", { ...u }); };
      const stamps = { checkedAt: update.checkedAt, retryAt: update.retryAt };
      // Inside the cooldown: no network call. A known update stays offered, otherwise "rateLimited" with retryAt.
      if (update.retryAt && Date.now() < update.retryAt) {
        setU(update.version ? { ...update, status: "available" } : { status: "rateLimited", current: MOCK_VERSION, ...stamps });
        return { ...update };
      }
      setU({ status: "checking", current: MOCK_VERSION, version: update.version, notes: update.notes, ...stamps });
      await wait(400);
      const flag = q.get("update");
      const found = q.has("update") && flag !== "uptodate" && flag !== "checkerror" && flag !== "ratelimited";
      const t = Date.now();
      const done = { checkedAt: t, retryAt: t + COOLDOWN_MS };
      if (flag === "checkerror") setU({ status: "error", current: MOCK_VERSION, error: "Could not check for updates: the update server did not answer.", ...done });
      else setU(found ? { status: "available", current: MOCK_VERSION, version: MOCK_NEXT, notes: "Faster switching and a clearer rank card.", ...done } : { status: "upToDate", current: MOCK_VERSION, ...done });
      return { ...update };
    },
    async installUpdate() {
      if (frozen || !update.version || ["checking", "downloading", "installing"].includes(update.status)) {
        if (!update.version) throw "There is no update to install right now.";
        return;
      }
      const setU = (u: UpdateState) => { update = u; emit("update-state", { ...u }); };
      const offered = { current: MOCK_VERSION, version: update.version, notes: update.notes };
      void (async () => {
        for (let got = 0; got <= MOCK_SIZE; got += MOCK_SIZE / 25) {
          setU({ status: "downloading", ...offered, progress: { downloaded: Math.round(got), total: MOCK_SIZE } });
          await wait(160);
        }
        setU({ status: "installing", ...offered }); // the real app restarts here; the mock stays
      })();
    },
    async on(event, cb) {
      const set = (subs[event] ??= new Set() as never) as Set<typeof cb>;
      set.add(cb);
      return () => { set.delete(cb); };
    },
  };
}
