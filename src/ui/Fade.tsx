import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";

/** Keeps its children mounted while they fade out, so a closing layer can animate (closes faster than it opens:
 *  the out duration matches --t-out in app.css). Children style themselves off [data-state="in" | "out"] on the wrapper. */
export function Fade({ show, children, className, outMs = 150 }: { show: boolean; children: ReactNode; className?: string; outMs?: number }) {
  const [mounted, setMounted] = useState(show);
  useEffect(() => {
    if (show) { setMounted(true); return; }
    const t = window.setTimeout(() => setMounted(false), outMs);
    return () => window.clearTimeout(t);
  }, [show, outMs]);
  if (!show && !mounted) return null; // mount in the same render as show, so children can measure themselves
  return <div className={"fade " + (className ?? "")} data-state={show ? "in" : "out"}>{children}</div>;
}

/** Crossfades between states: when `id` changes, the old content stays for a moment in the same grid cell and fades out
 *  while the new content fades in with a small rise (app.css .swap). Both layers share one cell, so the box takes the
 *  taller of the two and the text below never jumps. A re-render with the same id (a countdown) just updates in place. */
export function Swap({ id, children, className, outMs = 200 }: { id: string; children: ReactNode; className?: string; outMs?: number }) {
  const last = useRef<{ id: string; node: ReactNode }>({ id, node: children });
  const [leaving, setLeaving] = useState<{ id: string; node: ReactNode } | null>(null);
  // Declared first: on an id change it still sees the previous snapshot, before the effect below replaces it.
  useLayoutEffect(() => {
    if (last.current.id === id) return;
    setLeaving(last.current);
    const t = window.setTimeout(() => setLeaving(null), outMs);
    return () => window.clearTimeout(t);
  }, [id, outMs]);
  useLayoutEffect(() => { last.current = { id, node: children }; });
  return (
    <div className={"swap " + (className ?? "")}>
      {leaving && leaving.id !== id && <div key={"out-" + leaving.id} className="swap-out" aria-hidden="true">{leaving.node}</div>}
      <div key={"in-" + id} className="swap-in">{children}</div>
    </div>
  );
}
