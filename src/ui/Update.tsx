// Optional updates (engine: updater.rs). The engine checks on its own schedule and only reports; nothing installs until
// the user presses the update button. Two surfaces: a filled pill in the title bar's top-right stack while an update is
// offered or running, and a "Check for updates" row in Settings that moves through checking, then the answer, as a
// crossfade (never an instant flip). A build without an update endpoint reports "disabled": no pill, and the Settings
// row only says so.
import { useEffect, useState, type ReactNode } from "react";
import type { UpdateState } from "../api";
import { useApp } from "../store";
import { Swap } from "./Fade";
import { Icon } from "./Icon";

const MB = 1024 * 1024;

function percent(u: UpdateState): number | null {
  const p = u.progress;
  return p && p.total ? Math.min(100, Math.round((p.downloaded / p.total) * 100)) : null;
}

/** One line for the current state; the pill and the Settings row read the same words. */
export function updateText(u: UpdateState): string {
  switch (u.status) {
    case "checking": return "Checking for updates";
    case "available": return `Version ${u.version} available`;
    case "downloading": {
      const pc = percent(u);
      if (pc !== null) return `Downloading ${u.version}, ${pc}%`;
      return `Downloading ${u.version}, ${((u.progress?.downloaded ?? 0) / MB).toFixed(1)} MB`;
    }
    case "installing": return "Installing, the app restarts";
    case "error": return u.error || "The update did not work.";
    case "upToDate": return `You are up to date (v${u.current})`;
    case "rateLimited": return "Checked a moment ago";
    case "disabled": return "Updates are not set up in this build";
    default: return `Version ${u.current}`;
  }
}

/** Title bar pill, left of settings in the top-right stack: "Update to X" in the accent while an update is offered
 *  (pressing it installs), the download progress while it runs, and a short failure line with Retry after a failed
 *  install. The small cross hides that version for this session (Settings still offers it); there is no forced update. */
export function UpdatePill() {
  const live = useApp((s) => s.update);
  const dismissed = useApp((s) => s.updateDismissed);
  const install = useApp((s) => s.installUpdate);
  const set = useApp((s) => s.set);
  if (!live || !live.version) return null;
  // A background re-check (or a check inside the cooldown) while an update is offered keeps showing the offer.
  const u: UpdateState = live.status === "checking" || live.status === "rateLimited" ? { ...live, status: "available" } : live;
  const running = u.status === "downloading" || u.status === "installing";
  const offered = u.status === "available" || u.status === "error";
  if (!running && !offered) return null;
  if (offered && dismissed === u.version) return null;
  const pc = u.status === "installing" ? null : percent(u); // installing has no measurable progress: a drifting segment
  return (
    <div className={"update-pill " + u.status} role="status" aria-live="polite">
      {u.status === "available" && (
        <button className="update-main" title={u.notes || undefined} onClick={() => void install()}>
          <Icon name="download" size={13} stroke={2} /><span className="update-text">Update to {u.version}</span>
        </button>
      )}
      {u.status === "error" && (
        <>
          <Icon name="alert" size={13} />
          <span className="update-text" title={u.error || undefined}>Update to {u.version} failed</span>
          <button className="update-btn" onClick={() => void install()}>Retry</button>
        </>
      )}
      {running && (
        <>
          <Icon name="download" size={13} />
          <span className="update-text">{updateText(u)}</span>
          <span className={"update-line" + (pc === null ? " indef" : "")} aria-hidden="true">
            <i style={pc === null ? undefined : { transform: `scaleX(${pc / 100})` }} />
          </span>
        </>
      )}
      {offered && (
        <button className="update-x" aria-label="Hide until next start" title="Hide until next start" onClick={() => set({ updateDismissed: u.version ?? null })}>
          <Icon name="close" size={10} stroke={2.4} />
        </button>
      )}
    </div>
  );
}

/** Re-renders a few times a second until `until` (ms epoch) has passed, for a live countdown; returns the current time. */
function useNow(until: number | undefined): number {
  const [, tick] = useState(0);
  useEffect(() => {
    if (!until || until <= Date.now()) return;
    const t = window.setInterval(() => { tick((n) => n + 1); if (Date.now() >= until) window.clearInterval(t); }, 250);
    return () => window.clearInterval(t);
  }, [until]);
  return Date.now();
}

function checkedWhen(checkedAt: number | undefined, now: number): string {
  if (!checkedAt || now - checkedAt < 60_000) return "a moment ago";
  const min = Math.round((now - checkedAt) / 60_000);
  return min < 60 ? `${min} min ago` : "earlier";
}

/** Settings row. Pressing "Check for updates" disables the button and shows "Checking for updates" with the drifting line
 *  for at least 1.1 s (store.checkUpdate), then crossfades into the answer: up to date, an offer with Update now, a
 *  cooldown with a live countdown, or the error with Retry. The button stays off until the engine's retryAt. */
export function UpdateRow() {
  const u = useApp((s) => s.update);
  const manual = useApp((s) => s.updateChecking);
  const check = useApp((s) => s.checkUpdate);
  const install = useApp((s) => s.installUpdate);
  const now = useNow(u?.retryAt);
  if (!u) return null;
  if (u.status === "disabled") {
    return (
      <div className="update-row">
        <div className="update-head">
          <span className="update-label">Updates</span>
        </div>
        <div className="update-status">
          <span className="update-row-main">
            <span className="update-row-text" title={`Version ${u.current}`}>{updateText(u)}</span>
          </span>
        </div>
      </div>
    );
  }

  let status = u.status;
  if (manual || (status === "checking" && !u.version)) status = "checking";
  else if ((status === "checking" || status === "rateLimited") && u.version) status = "available";
  const cooling = !!u.retryAt && now < u.retryAt;
  const left = cooling ? Math.ceil((u.retryAt! - now) / 1000) : 0;
  const wait = cooling ? `You can check again in ${left} s` : undefined;
  const checkBtn = (label = "Check for updates") => (
    <button className="btn-grey sm" disabled={status === "checking" || cooling} title={status === "checking" ? undefined : wait} onClick={() => void check()}>{label}</button>
  );

  let id: string = status;
  let tone = "";
  let text: ReactNode = updateText({ ...u, status });
  let line: ReactNode = null;
  let action: ReactNode = checkBtn();
  switch (status) {
    case "checking":
      line = <span className="update-row-line indef" aria-hidden="true"><i /></span>;
      break;
    case "upToDate":
      tone = " ok";
      text = <><Icon name="check" size={13} stroke={2.2} />{text}</>;
      break;
    case "available":
      tone = " new";
      action = <button className="btn-orange sm" onClick={() => void install()}>Update now</button>;
      break;
    case "rateLimited":
      id = cooling ? "rateLimited" : "rateLimitedReady";
      text = cooling ? `Checked ${checkedWhen(u.checkedAt, now)}. Try again in ${left} s` : "You can check again now";
      break;
    case "error":
      tone = " err";
      text = <span className="update-row-clamp" title={u.error || undefined}>{text}</span>;
      action = u.version ? <button className="btn-orange sm" onClick={() => void install()}>Retry</button> : checkBtn("Retry");
      break;
    case "downloading":
    case "installing": {
      const pc = status === "installing" ? null : percent(u);
      line = (
        <span className={"update-row-line" + (pc === null ? " indef" : "")} aria-hidden="true">
          <i style={pc === null ? undefined : { transform: `scaleX(${pc / 100})` }} />
        </span>
      );
      action = null;
      break;
    }
  }
  // "Updates" and the button on one line, the state under it at the panel's full width. Only the state line crossfades;
  // its box holds two lines, so nothing below it moves.
  return (
    <div className="update-row">
      <div className="update-head">
        <span className="update-label">Updates</span>
        {action}
      </div>
      <Swap id={id} className="update-status">
        <span className="update-row-main">
          <span className={"update-row-text" + tone} aria-live="polite">{text}</span>
          {line}
        </span>
      </Swap>
    </div>
  );
}
