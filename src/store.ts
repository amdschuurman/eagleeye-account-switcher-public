import { create } from "zustand";
import { api, errorText, isTauri, type Account, type Category, type RankInfo, type Role, type RoleRank, type Settings, type SwitchStage, type UpdateState } from "./api";
import { bestRole, effective, score } from "./rank";

export interface CardNote { text: string; kind: "ok" | "warn" | "err" }

/** local = this window started it (switchTo); otherwise it came from the tray or another caller, seen only by events. */
export interface Switching { code: string; stage: SwitchStage; local: boolean; game?: string }

export type SortKey = "manual" | "best" | Role;
export type CategoryFilter = "all" | Category;
/** Rank range on one role (or the best role); from and to are rank scores (rank.ts), inclusive. */
export interface RankFilter { role: Role | "best"; from: number; to: number }
export interface HoverAt { code: string; left: number; top: number; right: number; bottom: number }
/** The app's pages: the platform picker and one page per platform (only Battle.net today). */
export type Page = "platforms" | "battlenet";

interface State {
  loaded: boolean;
  accounts: Account[];
  current: string | null;
  selected: string | null; // an account code, or "add" for the Add card
  switching: Switching | null;
  notes: Record<string, CardNote>;
  adding: { stage: "starting" | "waiting_login" | "captured"; message?: string } | null;
  tcno: number;
  importing: { done: number; total: number; label: string; running: boolean } | null;
  importDismissed: boolean;
  savingCurrent: boolean;
  settings: Settings | null;
  settingsOpen: boolean;
  toast: { id: number; text: string } | null;
  menu: { code: string; x: number; y: number } | null;
  renaming: string | null;
  confirmRemove: string | null;
  ranks: Record<string, RankInfo>;
  iconDir: string | null;
  category: CategoryFilter;
  sort: SortKey;
  filter: RankFilter | null;
  filterOpen: boolean;
  sortOpen: boolean;
  hover: HoverAt | null;
  editRanks: string | null;
  update: UpdateState | null;
  updateDismissed: string | null; // the offered version the user closed the title bar pill for (this session only)
  updateChecking: boolean; // a manual "Check for updates" is running; held at least CHECK_MIN_MS so the row never flickers
  page: Page;
  pageBack: Page[]; // browser-like history: back stack (last = where Back goes) and forward stack
  pageForward: Page[];
  navDir: 1 | -1; // direction of the last page change, for the slide

  init(): Promise<void>;
  refresh(): Promise<void>;
  select(code: string | null): void;
  switchTo(code: string): Promise<void>;
  add(): Promise<void>;
  cancelAdd(): Promise<void>;
  saveCurrent(): Promise<void>;
  importTcno(): Promise<void>;
  rename(code: string, label: string): Promise<void>;
  move(code: string, dir: -1 | 1): Promise<void>;
  remove(code: string): Promise<void>;
  saveSettings(s: Settings): Promise<void>;
  showToast(text: string): void;
  loadRanks(code?: string): Promise<void>;
  refreshRanks(code: string | null, force?: boolean): Promise<void>;
  setCategory(code: string, category: Category): Promise<void>;
  saveRanks(code: string, changes: Partial<Record<Role, RoleRank | null>>): Promise<void>;
  checkUpdate(): Promise<void>;
  installUpdate(): Promise<void>;
  navigate(page: Page): void;
  back(): void;
  forward(): void;
  set(p: Partial<State>): void;
}

const noteTimers = new Map<string, number>();
let toastTimer = 0;
let started = false; // init runs once (React StrictMode runs effects twice in dev)
let importRunning = false; // import-progress events that arrive after import_tcno resolved must not bring the banner back
let externalTimer = 0; // an outside switch whose final event never arrives must not block the window forever
const RANK_REFRESH_MS = 15 * 60_000;
const CHECK_MIN_MS = 1100; // "Checking for updates" stays up at least this long, even when the engine answers at once

export const useApp = create<State>((set, get) => {
  const note = (code: string, n: CardNote | null, ttl = 0) => {
    window.clearTimeout(noteTimers.get(code));
    set((s) => {
      const notes = { ...s.notes };
      if (n) notes[code] = n; else delete notes[code];
      return { notes };
    });
    if (n && ttl) noteTimers.set(code, window.setTimeout(() => note(code, null), ttl));
  };
  const fail = (e: unknown) => get().showToast(errorText(e));

  return {
    loaded: false,
    accounts: [],
    current: null,
    selected: null,
    switching: null,
    notes: {},
    adding: null,
    tcno: 0,
    importing: null,
    importDismissed: false,
    savingCurrent: false,
    settings: null,
    settingsOpen: false,
    toast: null,
    menu: null,
    renaming: null,
    confirmRemove: null,
    ranks: {},
    iconDir: null,
    category: "all",
    sort: "manual",
    filter: null,
    filterOpen: false,
    sortOpen: false,
    hover: null,
    editRanks: null,
    update: null,
    updateDismissed: null,
    updateChecking: false,
    // Opens on the Battle.net page with the platform picker one step back, like a browser that came from there.
    page: "battlenet",
    pageBack: ["platforms"],
    pageForward: [],
    navDir: 1,

    set: (p) => set(p),

    navigate(page) {
      const s = get();
      if (page === s.page) return;
      set({ page, pageBack: [...s.pageBack, s.page], pageForward: [], navDir: 1, hover: null, menu: null, sortOpen: false, filterOpen: false });
    },
    back() {
      const s = get();
      const to = s.pageBack[s.pageBack.length - 1];
      if (!to) return;
      set({ page: to, pageBack: s.pageBack.slice(0, -1), pageForward: [...s.pageForward, s.page], navDir: -1, hover: null, menu: null, sortOpen: false, filterOpen: false });
    },
    forward() {
      const s = get();
      const to = s.pageForward[s.pageForward.length - 1];
      if (!to) return;
      set({ page: to, pageForward: s.pageForward.slice(0, -1), pageBack: [...s.pageBack, s.page], navDir: 1, hover: null, menu: null, sortOpen: false, filterOpen: false });
    },

    async init() {
      if (started) return;
      started = true;
      const a = await api();
      await a.on("switch-progress", (p) => {
        const sw = get().switching;
        const final = p.stage === "done" || p.stage === "needs_login" || p.stage === "error";
        if (sw?.local) {
          if (sw.code === p.code && !final) set({ switching: { code: p.code, stage: p.stage, local: true, game: p.game } });
          return; // switchTo handles the end of its own switch
        }
        // A switch this window did not start (tray, another caller): same card feedback, then re-read who is signed in.
        window.clearTimeout(externalTimer);
        if (!final) {
          set({ switching: { code: p.code, stage: p.stage, local: false, game: p.game } });
          externalTimer = window.setTimeout(() => { if (get().switching?.local === false) { set({ switching: null }); void get().refresh(); } }, 60000);
          return;
        }
        set({ switching: null });
        if (p.stage === "done") note(p.code, { text: `Signed in, ${(p.ms / 1000).toFixed(1)} s`, kind: "ok" }, 6000);
        else if (p.stage === "needs_login") note(p.code, { text: "Log in once in Battle.net", kind: "warn" }, 15000);
        else note(p.code, { text: "Did not switch", kind: "err" }, 6000);
        void get().refresh();
      });
      await a.on("add-progress", (p) => {
        if (!get().adding) return;
        if (p.stage === "error") set({ adding: { stage: "waiting_login", message: p.message } });
        else set({ adding: { stage: p.stage, message: p.message } });
      });
      await a.on("import-progress", (p) => {
        if (!importRunning) return;
        if (p.status !== "start") void get().refresh(); // each imported account appears as soon as it is saved
        if (p.status !== "start" && p.total > 0 && p.done >= p.total) { set({ importing: null }); return; } // the last one is in
        set({ importing: { done: p.done, total: p.total, label: p.label, running: p.status === "start" } });
      });
      await a.on("update-state", (u) => set({ update: u }));
      a.updateState().then((u) => set((s) => (s.update ? {} : { update: u })), () => {}); // an engine without updates: no pill
      await a.on("ranks-updated", (p) => void get().loadRanks(p.code));
      await a.on("ranks-error", (p) => {
        get().showToast(p.error);
        void get().loadRanks(p.code ?? undefined);
      });
      try {
        const [accounts, current, tcno, settings] = await Promise.all([a.listAccounts(), a.currentAccount(), a.tcnoAvailable(), a.getSettings()]);
        set({ accounts, current, tcno, settings, loaded: true, selected: current && accounts.some((x) => x.code === current) ? current : accounts[0]?.code ?? "add" });
      } catch (e) {
        set({ loaded: true });
        fail(e);
      }
      // Ranks are extra: an engine without them, or no icons yet, leaves the switcher fully working.
      a.iconDir().then((iconDir) => set({ iconDir }), () => {});
      void get().loadRanks();
      // The engine refreshes in the background (1 h cache per account) and reports each one with ranks-updated.
      const bg = () => { void a.refreshRanks(null, false).catch(() => {}); };
      bg();
      window.setInterval(bg, RANK_REFRESH_MS);
    },

    async refresh() {
      const a = await api();
      try {
        const [accounts, current] = await Promise.all([a.listAccounts(), a.currentAccount()]);
        set((s) => ({ accounts, current, selected: s.selected === "add" || accounts.some((x) => x.code === s.selected) ? s.selected : accounts[0]?.code ?? "add" }));
      } catch {
        /* a failed background refresh keeps the last list; the next one tries again */
      }
    },

    select: (code) => set({ selected: code }),

    async switchTo(code) {
      if (get().switching || get().adding || get().importing) return; // one action at a time, extra clicks ignored
      const a = await api();
      note(code, null);
      set({ switching: { code, stage: "closing", local: true }, selected: code });
      try {
        const r = await a.switchAccount(code);
        if (r.status === "done") {
          note(code, { text: `Signed in, ${(r.ms / 1000).toFixed(1)} s`, kind: "ok" }, 6000);
          set({ current: code });
          if (get().settings?.closeAfterSwitch && isTauri) {
            window.setTimeout(async () => { const { getCurrentWindow } = await import("@tauri-apps/api/window"); await getCurrentWindow().close(); }, 900);
          }
        } else if (r.status === "needs_login") {
          note(code, { text: "Log in once in Battle.net", kind: "warn" }, 15000);
        } else {
          note(code, { text: "Did not switch", kind: "err" }, 6000);
          if (r.message) get().showToast(r.message);
        }
      } catch (e) {
        note(code, { text: "Did not switch", kind: "err" }, 6000);
        fail(e);
      } finally {
        set({ switching: null });
        void get().refresh();
      }
    },

    async add() {
      if (get().switching || get().adding || get().importing) return;
      const a = await api();
      set({ adding: { stage: "starting" }, menu: null });
      try {
        const acct = await a.addAccount();
        set({ adding: { stage: "captured" } });
        await get().refresh();
        set({ selected: acct.code });
        note(acct.code, { text: "Saved", kind: "ok" }, 5000);
        window.setTimeout(() => set({ adding: null }), 700);
      } catch (e) {
        set({ adding: null });
        const msg = errorText(e);
        if (!/cancel/i.test(msg)) get().showToast(msg);
      }
    },

    async cancelAdd() {
      const a = await api();
      try { await a.cancelAdd(); } catch (e) { fail(e); }
    },

    async saveCurrent() {
      if (get().savingCurrent || get().switching || get().adding) return;
      const a = await api();
      set({ savingCurrent: true });
      try {
        const acct = await a.saveCurrent();
        await get().refresh();
        set({ selected: acct.code });
        note(acct.code, { text: "Saved", kind: "ok" }, 5000);
      } catch (e) {
        fail(e);
      } finally {
        set({ savingCurrent: false });
      }
    },

    async importTcno() {
      if (get().importing || get().switching || get().adding) return;
      const a = await api();
      set({ importing: { done: 0, total: get().tcno, label: "", running: false } });
      importRunning = true;
      try {
        const r = await a.importTcno();
        const parts = [`Imported ${r.imported}`];
        if (r.needsLogin) parts.push(`${r.needsLogin} need a login once`);
        if (r.skipped) parts.push(`${r.skipped} skipped`);
        get().showToast(parts.join(", ") + ".");
        set({ tcno: await a.tcnoAvailable().catch(() => 0) });
      } catch (e) {
        fail(e);
      } finally {
        importRunning = false;
        set({ importing: null });
        await get().refresh();
      }
    },

    async rename(code, label) {
      const clean = label.trim();
      set({ renaming: null });
      const prev = get().accounts.find((x) => x.code === code);
      if (!clean || !prev || clean === prev.label) return;
      set((s) => ({ accounts: s.accounts.map((x) => (x.code === code ? { ...x, label: clean } : x)) }));
      try { await (await api()).renameAccount(code, clean); } catch (e) { fail(e); }
      void get().refresh();
    },

    async move(code, dir) {
      const list = [...get().accounts];
      const i = list.findIndex((x) => x.code === code);
      const j = i + dir;
      if (i < 0 || j < 0 || j >= list.length) return;
      [list[i], list[j]] = [list[j], list[i]];
      set({ accounts: list.map((x, k) => ({ ...x, order: k })) });
      try { await (await api()).reorderAccounts(list.map((x) => x.code)); } catch (e) { fail(e); }
      void get().refresh();
    },

    async remove(code) {
      set({ confirmRemove: null });
      const list = get().accounts;
      const i = list.findIndex((x) => x.code === code);
      try {
        await (await api()).removeAccount(code);
        const rest = list.filter((x) => x.code !== code);
        set({ accounts: rest, selected: rest[Math.min(i, rest.length - 1)]?.code ?? "add" });
      } catch (e) { fail(e); }
      void get().refresh();
    },

    async saveSettings(s) {
      set({ settings: s });
      try { await (await api()).setSettings(s); } catch (e) { fail(e); }
    },

    async loadRanks(code) {
      const a = await api();
      try {
        if (code) {
          const info = await a.getRanks(code);
          set((s) => {
            const ranks = { ...s.ranks };
            if (info) ranks[code] = info; else delete ranks[code];
            return { ranks };
          });
        } else {
          set({ ranks: await a.getAllRanks() });
        }
      } catch {
        /* no ranks is a quiet state: the cards simply show none */
      }
    },

    async refreshRanks(code, force) {
      try { await (await api()).refreshRanks(code, force); } catch (e) { fail(e); }
      void get().loadRanks(code ?? undefined);
    },

    async setCategory(code, category) {
      set((s) => ({ accounts: s.accounts.map((x) => (x.code === code ? { ...x, category } : x)) }));
      try { await (await api()).setCategory(code, category); } catch (e) { fail(e); }
      void get().refresh();
    },

    async saveRanks(code, changes) {
      set({ editRanks: null });
      const a = await api();
      try {
        for (const [role, rank] of Object.entries(changes) as Array<[Role, RoleRank | null]>) await a.setRankOverride(code, role, rank);
      } catch (e) { fail(e); }
      void get().loadRanks(code);
    },

    async checkUpdate() {
      const s = get();
      if (s.updateChecking || (s.update?.retryAt && Date.now() < s.update.retryAt)) return; // the UI's own cooldown guard
      set({ updateChecking: true });
      try {
        const [u] = await Promise.all([(await api()).checkUpdate(), new Promise((r) => window.setTimeout(r, CHECK_MIN_MS))]);
        set({ update: u });
      } catch (e) {
        // A check that could not run: say so in the row, unless an offer is already known (then keep it, toast instead).
        const u = get().update;
        if (u?.version) fail(e);
        else set({ update: { ...(u ?? { current: "" }), status: "error", error: errorText(e) } });
      } finally {
        set({ updateChecking: false });
      }
    },

    async installUpdate() {
      set({ updateDismissed: null });
      try { await (await api()).installUpdate(); } catch (e) { fail(e); }
    },

    showToast(text) {
      window.clearTimeout(toastTimer);
      set({ toast: { id: Date.now(), text } });
      toastTimer = window.setTimeout(() => set({ toast: null }), 4200);
    },
  };
});

/** The rank an account is sorted and filtered by for one key; -1 = unranked (always last, never inside a range). */
export function rankScore(info: RankInfo | undefined, key: Role | "best"): number {
  if (!info || info.privateProfile) return -1;
  if (key === "best") return score(bestRole(info)?.rank);
  return score(effective(info.roles[key]));
}

export function categoryOf(a: Account): Category {
  return a.category ?? "smurf";
}

/** What the grid draws: the category chip, then the rank filter, then the sort (ties keep the manual order). */
export function visibleAccounts(s: Pick<State, "accounts" | "ranks" | "category" | "sort" | "filter">): Account[] {
  let list = s.accounts;
  if (s.category !== "all") list = list.filter((a) => categoryOf(a) === s.category);
  const f = s.filter;
  if (f) {
    const lo = Math.min(f.from, f.to), hi = Math.max(f.from, f.to);
    list = list.filter((a) => { const v = rankScore(s.ranks[a.code], f.role); return v >= lo && v <= hi; });
  }
  if (s.sort !== "manual") {
    const key = s.sort === "best" ? "best" : s.sort;
    list = list.map((a, i) => ({ a, i, v: rankScore(s.ranks[a.code], key) })).sort((x, y) => y.v - x.v || x.i - y.i).map((x) => x.a);
  }
  return list;
}
