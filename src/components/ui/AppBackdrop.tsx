import { useEffect, useState } from 'react';

/**
 * The one place the photo gradient is painted. It sits behind the whole UI so panels only need a flat scrim.
 * With the wgpu renderer the editor canvas is a transparent "hole" in the page, so the layer is clipped to leave
 * that rectangle empty. The hole is tracked with a ResizeObserver (no per-frame layout reads).
 */
export default function AppBackdrop({ holeActive }: { holeActive: boolean }) {
  const [clip, setClip] = useState<string | undefined>(undefined);

  useEffect(() => {
    if (!holeActive) {
      setClip(undefined);
      return;
    }
    let last = '';
    let raf = 0;
    const measure = () => {
      raf = 0;
      const el = document.querySelector('[data-wgpu-hole]');
      if (!el) return;
      const r = el.getBoundingClientRect();
      const radius = parseFloat(getComputedStyle(el).borderTopLeftRadius) || 0;
      const key = `${r.left}|${r.top}|${r.right}|${r.bottom}|${radius}|${window.innerWidth}|${window.innerHeight}`;
      if (key === last) return;
      last = key;
      const rx = Math.min(radius, r.width / 2, r.height / 2);
      const inner =
        `M${r.left + rx},${r.top}H${r.right - rx}A${rx},${rx} 0 0 1 ${r.right},${r.top + rx}V${r.bottom - rx}` +
        `A${rx},${rx} 0 0 1 ${r.right - rx},${r.bottom}H${r.left + rx}A${rx},${rx} 0 0 1 ${r.left},${r.bottom - rx}` +
        `V${r.top + rx}A${rx},${rx} 0 0 1 ${r.left + rx},${r.top}Z`;
      setClip(`path(evenodd, 'M0,0H${window.innerWidth}V${window.innerHeight}H0Z ${inner}')`);
    };
    const schedule = () => {
      if (!raf) raf = requestAnimationFrame(measure);
    };

    const el = document.querySelector('[data-wgpu-hole]');
    const observer = new ResizeObserver(schedule);
    if (el) observer.observe(el);
    observer.observe(document.documentElement);
    window.addEventListener('resize', schedule);
    // Panel resizes and layout animations move the hole without resizing it: re-measure when they settle.
    document.addEventListener('transitionend', schedule, true);
    document.addEventListener('pointerup', schedule, true);
    schedule();
    return () => {
      observer.disconnect();
      window.removeEventListener('resize', schedule);
      document.removeEventListener('transitionend', schedule, true);
      document.removeEventListener('pointerup', schedule, true);
      if (raf) cancelAnimationFrame(raf);
    };
  }, [holeActive]);

  return (
    <div
      aria-hidden
      className="fixed inset-0 pointer-events-none"
      style={{ zIndex: -1, backgroundImage: 'var(--photo-gradient)', clipPath: clip }}
    />
  );
}
