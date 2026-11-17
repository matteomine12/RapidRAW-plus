import { useEffect, useState } from 'react';

/**
 * With the wgpu renderer the editor canvas is a transparent "hole" in the page. Everything around it
 * must still be painted, so this layer draws the photo gradient everywhere except that hole.
 */
export default function AppBackdrop({ holeActive }: { holeActive: boolean }) {
  const [clip, setClip] = useState<string | undefined>(undefined);

  useEffect(() => {
    if (!holeActive) {
      setClip(undefined);
      return;
    }
    let raf = 0;
    let last = '';
    const tick = () => {
      const el = document.querySelector('[data-wgpu-hole]');
      if (el) {
        const r = el.getBoundingClientRect();
        const radius = parseFloat(getComputedStyle(el).borderTopLeftRadius) || 0;
        const key = `${r.left}|${r.top}|${r.right}|${r.bottom}|${radius}`;
        if (key !== last) {
          last = key;
          const w = window.innerWidth;
          const h = window.innerHeight;
          const rx = Math.min(radius, r.width / 2, r.height / 2);
          const inner =
            `M${r.left + rx},${r.top}H${r.right - rx}A${rx},${rx} 0 0 1 ${r.right},${r.top + rx}V${r.bottom - rx}` +
            `A${rx},${rx} 0 0 1 ${r.right - rx},${r.bottom}H${r.left + rx}A${rx},${rx} 0 0 1 ${r.left},${r.bottom - rx}` +
            `V${r.top + rx}A${rx},${rx} 0 0 1 ${r.left + rx},${r.top}Z`;
          setClip(`path(evenodd, 'M0,0H${w}V${h}H0Z ${inner}')`);
        }
      }
      raf = requestAnimationFrame(tick);
    };
    tick();
    return () => cancelAnimationFrame(raf);
  }, [holeActive]);

  if (!holeActive) return null;
  return (
    <div
      aria-hidden
      className="fixed inset-0 pointer-events-none"
      style={{
        zIndex: -1,
        backgroundImage: 'linear-gradient(var(--app-scrim), var(--app-scrim)), var(--photo-gradient)',
        clipPath: clip,
      }}
    />
  );
}
