import logo from "../assets/brand/logo.svg";
import { isTauri } from "../api";
import { useApp } from "../store";
import { Icon } from "./Icon";
import { UpdatePill } from "./Update";

async function win() {
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  return getCurrentWindow();
}

/** The frameless window's own title bar: the EagleEye mark top left (original black-body mark on the dark ground,
 *  never recoloured), the app name, back and forward buttons, and the top-right stack:
 *  the update pill when an update is offered, settings, minimise and close (one row of window buttons).
 *  The bar itself is the drag region; the logo and name ignore the pointer so a drag that starts on them still moves the
 *  window. */
export function TitleBar() {
  const settingsOpen = useApp((s) => s.settingsOpen);
  const canBack = useApp((s) => s.pageBack.length > 0);
  const canForward = useApp((s) => s.pageForward.length > 0);
  const { set, back, forward } = useApp.getState();
  return (
    <header className="titlebar" data-tauri-drag-region>
      <img src={logo} alt="EagleEye" className="brand-mark" draggable={false} />
      <span className="brand-name">Account Switcher</span>
      <div className="history-btns">
        <button disabled={!canBack} onClick={back} aria-label="Back" title="Back (Alt+Left)"><Icon name="arrowLeft" size={14} /></button>
        <button disabled={!canForward} onClick={forward} aria-label="Forward" title="Forward (Alt+Right)"><Icon name="arrowRight" size={14} /></button>
      </div>
      <div className="titlebar-spacer" data-tauri-drag-region />
      <div className="winctl">
        <UpdatePill />
        <button className={"tb-settings" + (settingsOpen ? " open" : "")} aria-label="Settings" title="Settings" aria-expanded={settingsOpen}
          onClick={() => set({ settingsOpen: !settingsOpen, menu: null })}>
          <Icon name="settings" size={13} />
        </button>
        <button aria-label="Minimize" onClick={async () => { if (isTauri) await (await win()).minimize(); }}><Icon name="minimize" size={12} /></button>
        <button aria-label="Close" className="close" onClick={async () => { if (isTauri) await (await win()).close(); }}><Icon name="close" size={12} /></button>
      </div>
    </header>
  );
}
