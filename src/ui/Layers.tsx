import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { categoryOf, useApp } from "../store";
import { Fade } from "./Fade";
import { Icon } from "./Icon";
import { UpdateRow } from "./Update";

/** Last non-null value, so a layer keeps its content while it fades out after the state that fed it is cleared. */
function useSticky<T>(v: T | null): T | null {
  const last = useRef(v);
  if (v !== null) last.current = v;
  return last.current;
}

/** Add account: Battle.net's own login window does the work; this layer says so and offers Cancel. */
export function AddOverlay() {
  const adding = useApp((s) => s.adding);
  const shown = useSticky(adding);
  const cancel = useApp((s) => s.cancelAdd);
  const captured = shown?.stage === "captured";
  const btn = useRef<HTMLButtonElement>(null);
  useEffect(() => { if (adding) btn.current?.focus(); }, [adding]);
  useEffect(() => {
    if (!adding) return;
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") void cancel(); };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [adding, cancel]);
  return (
    <Fade show={!!adding} className="layer add-layer">
      <div className="add-panel" role="dialog" aria-modal="true" aria-labelledby="add-title">
        <h2 id="add-title">{captured ? "Account saved" : shown?.stage === "starting" ? "Opening Battle.net" : "Log in to Battle.net"}</h2>
        {captured ? (
          <p>You can log in to it from here with one click from now on.</p>
        ) : (
          <>
            <p>Battle.net is open in its own window. Log in there with the account you want to add, and tick <b>Keep me logged in</b>.</p>
            <p className="muted">This window picks it up by itself as soon as you are in.</p>
          </>
        )}
        {shown?.message && !captured && <p className="add-msg">{shown.message}</p>}
        <div className={"line-indef" + (captured ? " done" : "")} aria-hidden="true"><i /></div>
        {!captured && <button ref={btn} className="btn-grey" onClick={() => void cancel()}>Cancel</button>}
      </div>
    </Fade>
  );
}

/** Right-click menu on a card: the masked email as a quiet header (it is never on the card itself), Rename, Edit ranks,
 *  Refresh ranks, Set as Main / Set as Smurf, Move left, Move right, Remove. Stays inside the window. Moving only makes
 *  sense in the manual order with every account visible, so it is off while a sort, category or filter is active. */
export function CardMenu() {
  const menu = useApp((s) => s.menu);
  const shown = useSticky(menu);
  const accounts = useApp((s) => s.accounts);
  const busy = useApp((s) => !!s.switching || !!s.importing);
  const set = useApp((s) => s.set);
  const move = useApp((s) => s.move);
  const setCategory = useApp((s) => s.setCategory);
  const refreshRanks = useApp((s) => s.refreshRanks);
  const manualView = useApp((s) => s.sort === "manual" && s.category === "all" && !s.filter);
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ x: 0, y: 0 });

  useLayoutEffect(() => {
    if (!menu || !ref.current) return;
    const r = ref.current.getBoundingClientRect();
    setPos({ x: Math.min(menu.x, window.innerWidth - r.width - 8), y: Math.min(menu.y, window.innerHeight - r.height - 8) });
  }, [menu]);

  useEffect(() => {
    if (!menu) return;
    const close = (e: Event) => { if (!(e.target instanceof Node && ref.current?.contains(e.target))) set({ menu: null }); };
    const key = (e: KeyboardEvent) => { if (e.key === "Escape") set({ menu: null }); };
    window.addEventListener("mousedown", close, true);
    window.addEventListener("blur", close);
    window.addEventListener("keydown", key);
    return () => { window.removeEventListener("mousedown", close, true); window.removeEventListener("blur", close); window.removeEventListener("keydown", key); };
  }, [menu, set]);

  const i = shown ? accounts.findIndex((a) => a.code === shown.code) : -1;
  const acct = i >= 0 ? accounts[i] : undefined;
  const cat = acct ? categoryOf(acct) : null;
  const pick = (fn: () => void) => () => { set({ menu: null }); fn(); };
  return (
    <Fade show={!!menu} outMs={120} className="menu-layer">
      <div ref={ref} className="ctx-menu" role="menu" style={{ left: pos.x, top: pos.y }}>
        {acct?.emailMasked && <div className="ctx-head" title="Battle.net login">{acct.emailMasked}</div>}
        <button role="menuitem" onClick={pick(() => set({ renaming: shown!.code }))}><Icon name="pencil" size={14} /> Rename</button>
        <button role="menuitem" onClick={pick(() => set({ editRanks: shown!.code }))}><Icon name="medal" size={14} /> Edit ranks</button>
        <button role="menuitem" onClick={pick(() => void refreshRanks(shown!.code, true))}><Icon name="refresh" size={14} /> Refresh ranks</button>
        <div className="ctx-sep" />
        <button role="menuitemradio" aria-checked={cat === "main"} disabled={cat === "main"} onClick={pick(() => void setCategory(shown!.code, "main"))}>
          <Icon name="tag" size={14} /> Set as Main{cat === "main" && <span className="ctx-now">Now</span>}
        </button>
        <button role="menuitemradio" aria-checked={cat === "smurf"} disabled={cat === "smurf"} onClick={pick(() => void setCategory(shown!.code, "smurf"))}>
          <Icon name="tag" size={14} /> Set as Smurf{cat === "smurf" && <span className="ctx-now">Now</span>}
        </button>
        <div className="ctx-sep" />
        <button role="menuitem" disabled={!manualView || i <= 0} onClick={pick(() => void move(shown!.code, -1))}><Icon name="arrowLeft" size={14} /> Move left</button>
        <button role="menuitem" disabled={!manualView || i < 0 || i >= accounts.length - 1} onClick={pick(() => void move(shown!.code, 1))}><Icon name="arrowRight" size={14} /> Move right</button>
        <div className="ctx-sep" />
        <button role="menuitem" className="danger" disabled={busy} onClick={pick(() => set({ confirmRemove: shown!.code }))}><Icon name="trash" size={14} /> Remove</button>
      </div>
    </Fade>
  );
}

export function ConfirmRemove() {
  const code = useApp((s) => s.confirmRemove);
  const shownCode = useSticky(code);
  const acct = useApp((s) => s.accounts.find((a) => a.code === shownCode));
  const { remove, set } = useApp.getState();
  const btn = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (!code) return;
    btn.current?.focus();
    const key = (e: KeyboardEvent) => { if (e.key === "Escape") set({ confirmRemove: null }); };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, [code, set]);
  return (
    <Fade show={!!code} className="layer dim">
      <div className="dialog" role="alertdialog" aria-modal="true" aria-labelledby="rm-title">
        <h2 id="rm-title">Remove {acct?.label ?? "this account"}?</h2>
        <p>Its saved login is deleted from this PC. The Battle.net account itself is not touched, and you can add it again later.</p>
        <div className="dialog-foot">
          <button className="btn-grey" onClick={() => set({ confirmRemove: null })}>Keep it</button>
          <button ref={btn} className="btn-danger" onClick={() => shownCode && void remove(shownCode)}>Remove</button>
        </div>
      </div>
    </Fade>
  );
}

/** Small settings panel under the cog: Battle.net folder, what happens after a switch, and updates. */
export function SettingsPanel() {
  const open = useApp((s) => s.settingsOpen);
  const settings = useApp((s) => s.settings);
  const save = useApp((s) => s.saveSettings);
  const set = useApp((s) => s.set);
  const [path, setPath] = useState(settings?.battlenetPath ?? "");
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => { if (open) setPath(settings?.battlenetPath ?? ""); }, [open, settings?.battlenetPath]);
  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => {
      const t = e.target as HTMLElement;
      if (!ref.current?.contains(t) && !t.closest?.(".tb-settings")) set({ settingsOpen: false });
    };
    const key = (e: KeyboardEvent) => { if (e.key === "Escape") set({ settingsOpen: false }); };
    window.addEventListener("mousedown", close, true);
    window.addEventListener("keydown", key);
    return () => { window.removeEventListener("mousedown", close, true); window.removeEventListener("keydown", key); };
  }, [open, set]);
  // Empty means "find it automatically" (the engine reads the install location from the registry).
  const commitPath = () => {
    if (!settings) return;
    const next = path.trim() || null;
    if (next !== (settings.battlenetPath ?? null)) void save({ ...settings, battlenetPath: next });
  };
  return (
    <Fade show={open} outMs={120} className="menu-layer">
      <div ref={ref} className="settings-panel" role="dialog" aria-label="Settings">
        <h3>Settings</h3>
        <label className="field">
          <span>Battle.net folder</span>
          <span className="input-wrap">
            <Icon name="folder" size={14} />
            <input className="input" value={path} spellCheck={false} placeholder="Found automatically" onChange={(e) => setPath(e.target.value)} onBlur={commitPath}
              onKeyDown={(e) => { if (e.key === "Enter") (e.target as HTMLInputElement).blur(); }} />
          </span>
        </label>
        <label className="check">
          <input type="checkbox" checked={!!settings?.closeAfterSwitch} disabled={!settings}
            onChange={(e) => settings && void save({ ...settings, closeAfterSwitch: e.target.checked })} />
          <span className="box"><Icon name="check" size={12} stroke={2.6} /></span>
          <span>Close this window after a switch</span>
        </label>
        <div className="settings-sep" />
        <UpdateRow />
      </div>
    </Fade>
  );
}

export function Toast() {
  const toast = useApp((s) => s.toast);
  const shown = useSticky(toast);
  return (
    <Fade show={!!toast} className="toast-layer">
      <div className="toast" role="status" aria-live="polite" key={shown?.id}>{shown?.text}</div>
    </Fade>
  );
}
