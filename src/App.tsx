import { useEffect } from "react";
import { useApp } from "./store";
import { TitleBar } from "./ui/TitleBar";
import { AddOverlay, CardMenu, ConfirmRemove, SettingsPanel, Toast } from "./ui/Layers";
import { Pages } from "./ui/Pages";
import { HoverCard } from "./ui/HoverCard";
import { EditRanks } from "./ui/EditRanks";

// Battle.net can be switched inside Battle.net itself; current_account is cheap (about 15 ms), so read it every 5 s.
const POLL_MS = 5000;

export function App() {
  const loaded = useApp((s) => s.loaded);

  useEffect(() => {
    void useApp.getState().init();
    // Battle.net can change account behind our back (signed out, logged in by hand): re-read now and then, and on focus.
    const tick = () => {
      const s = useApp.getState();
      if (document.visibilityState === "visible" && !s.switching && !s.adding && !s.importing) void s.refresh();
    };
    const t = window.setInterval(tick, POLL_MS);
    window.addEventListener("focus", tick);
    return () => { window.clearInterval(t); window.removeEventListener("focus", tick); };
  }, []);

  // Page history like a browser: Alt+Left / Alt+Right and the mouse's back and forward buttons (3 and 4). The webview's
  // own history navigation on those inputs is cancelled; there is only one document.
  useEffect(() => {
    const blocked = () => { const s = useApp.getState(); return !!(s.adding || s.confirmRemove || s.editRanks || s.renaming); };
    const onKey = (e: KeyboardEvent) => {
      if (!e.altKey || (e.key !== "ArrowLeft" && e.key !== "ArrowRight")) return;
      e.preventDefault();
      if (blocked()) return;
      if (e.key === "ArrowLeft") useApp.getState().back(); else useApp.getState().forward();
    };
    const onDown = (e: MouseEvent) => { if (e.button === 3 || e.button === 4) e.preventDefault(); };
    const onUp = (e: MouseEvent) => {
      if (e.button !== 3 && e.button !== 4) return;
      e.preventDefault();
      if (blocked()) return;
      if (e.button === 3) useApp.getState().back(); else useApp.getState().forward();
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("mousedown", onDown);
    window.addEventListener("mouseup", onUp);
    return () => { window.removeEventListener("keydown", onKey); window.removeEventListener("mousedown", onDown); window.removeEventListener("mouseup", onUp); };
  }, []);

  return (
    <div className="app">
      <TitleBar />
      <main className={"content" + (loaded ? " ready" : "")} onContextMenu={(e) => e.preventDefault()}>
        <Pages />
      </main>
      <AddOverlay />
      <ConfirmRemove />
      <EditRanks />
      <HoverCard />
      <CardMenu />
      <SettingsPanel />
      <Toast />
    </div>
  );
}
