import type { Api, EventMap } from "./types";

export * from "./types";

/** True inside the Tauri window. A plain browser (vite dev, screenshots) gets the mock engine instead. */
export const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

async function tauriApi(): Promise<Api> {
  const { invoke, convertFileSrc } = await import("@tauri-apps/api/core");
  const { listen } = await import("@tauri-apps/api/event");
  // Tauri maps JS camelCase argument keys onto the Rust parameter names (snake_case), so every key here is the
  // Rust parameter name in camelCase: code, label, codes, settings.
  return {
    listAccounts: () => invoke("list_accounts"),
    currentAccount: () => invoke("current_account"),
    switchAccount: (code) => invoke("switch_account", { code }),
    addAccount: () => invoke("add_account"),
    cancelAdd: () => invoke("cancel_add"),
    saveCurrent: () => invoke("save_current"),
    importTcno: () => invoke("import_tcno"),
    tcnoAvailable: () => invoke("tcno_available"),
    renameAccount: (code, label) => invoke("rename_account", { code, label }),
    removeAccount: (code) => invoke("remove_account", { code }),
    reorderAccounts: (codes) => invoke("reorder_accounts", { codes }),
    getSettings: () => invoke("get_settings"),
    setSettings: (settings) => invoke("set_settings", { settings }),
    getRanks: (code) => invoke("get_ranks", { code }),
    getAllRanks: () => invoke("get_all_ranks"),
    refreshRanks: (code, force) => invoke("refresh_ranks", { code, force: !!force }),
    setRankOverride: (code, role, rank) => invoke("set_rank_override", { code, role, rank }),
    setCategory: (code, category) => invoke("set_category", { code, category }),
    iconDir: () => invoke("icon_dir"),
    updateState: () => invoke("update_state"),
    checkUpdate: () => invoke("check_update"),
    installUpdate: () => invoke("install_update"),
    // icon_dir() is a Windows folder; the asset protocol must allow it (tauri.conf.json assetProtocol scope).
    iconUrl: (dir, file) => convertFileSrc(joinPath(dir, file)),
    avatarUrl: (dir, file) => convertFileSrc(joinPath(joinPath(dir, "avatars"), file)),
    on: <K extends keyof EventMap>(event: K, cb: (p: EventMap[K]) => void) => listen<EventMap[K]>(event, (e) => cb(e.payload)),
  };
}

/** Joins with the folder's own separator (a Windows folder from the engine uses backslashes). */
function joinPath(dir: string, name: string): string {
  return dir.replace(/[\\/]+$/, "") + (dir.includes("\\") ? "\\" : "/") + name;
}

let ready: Promise<Api> | null = null;

/** The engine, resolved once. */
export function api(): Promise<Api> {
  if (!ready) ready = isTauri ? tauriApi() : import("./mock").then((m) => m.createMockApi());
  return ready;
}

/** A rejected invoke carries the engine's message as a plain string; anything else is turned into one. */
export function errorText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e && typeof e === "object" && "message" in e && typeof (e as { message: unknown }).message === "string") return (e as { message: string }).message;
  return "Something went wrong.";
}
