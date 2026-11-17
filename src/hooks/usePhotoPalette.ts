import { useEffect, useRef } from 'react';
import { useEditorStore } from '../store/useEditorStore';
import { useLibraryStore } from '../store/useLibraryStore';
import { useProcessStore } from '../store/useProcessStore';

const SAMPLE = 24;
const HUE_BINS = 12;

interface Palette {
  hues: [number, number, number];
  sats: [number, number, number];
}

function rgbToHsl(r: number, g: number, b: number): [number, number, number] {
  r /= 255;
  g /= 255;
  b /= 255;
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const l = (max + min) / 2;
  const d = max - min;
  if (d === 0) return [0, 0, l];
  const s = d / (1 - Math.abs(2 * l - 1));
  let h: number;
  if (max === r) h = ((g - b) / d) % 6;
  else if (max === g) h = (b - r) / d + 2;
  else h = (r - g) / d + 4;
  return [(h * 60 + 360) % 360, s, l];
}

/** Picks three representative hues from an image, weighted by how colourful each pixel is. */
function extractPalette(img: HTMLImageElement): Palette | null {
  const canvas = document.createElement('canvas');
  canvas.width = SAMPLE;
  canvas.height = SAMPLE;
  const ctx = canvas.getContext('2d', { willReadFrequently: true });
  if (!ctx) return null;
  ctx.drawImage(img, 0, 0, SAMPLE, SAMPLE);
  let data: Uint8ClampedArray;
  try {
    data = ctx.getImageData(0, 0, SAMPLE, SAMPLE).data;
  } catch {
    return null;
  }

  const weight = new Array<number>(HUE_BINS).fill(0);
  const hueSum = new Array<number>(HUE_BINS).fill(0);
  const satSum = new Array<number>(HUE_BINS).fill(0);
  let totalSat = 0;

  for (let i = 0; i < data.length; i += 4) {
    const [h, s, l] = rgbToHsl(data[i], data[i + 1], data[i + 2]);
    const w = s * (1 - Math.abs(2 * l - 1)) + 0.002;
    const bin = Math.floor(h / (360 / HUE_BINS)) % HUE_BINS;
    weight[bin] += w;
    hueSum[bin] += h * w;
    satSum[bin] += s * w;
    totalSat += s;
  }

  const ranked = weight
    .map((w, bin) => ({ w, bin }))
    .sort((a, b) => b.w - a.w)
    .filter((e) => e.w > 0);
  if (ranked.length === 0) return null;

  const meanSat = totalSat / (SAMPLE * SAMPLE);
  const picks: Array<{ hue: number; sat: number }> = [];
  for (const { bin, w } of ranked) {
    const hue = hueSum[bin] / w;
    if (picks.every((p) => Math.min(Math.abs(p.hue - hue), 360 - Math.abs(p.hue - hue)) > 28)) {
      picks.push({ hue, sat: satSum[bin] / w });
    }
    if (picks.length === 3) break;
  }
  while (picks.length < 3) {
    const base = picks[picks.length - 1] ?? { hue: 220, sat: 0.4 };
    picks.push({ hue: (base.hue + 40) % 360, sat: base.sat });
  }

  // Near-monochrome photos keep a quiet, desaturated wash instead of inventing colour.
  const cap = meanSat < 0.08 ? 0.12 : 0.7;
  const sat = (s: number) => Math.min(cap, Math.max(cap === 0.12 ? 0.04 : 0.35, s));
  return {
    hues: [picks[0].hue, picks[1].hue, picks[2].hue],
    sats: [sat(picks[0].sat), sat(picks[1].sat), sat(picks[2].sat)],
  };
}

/**
 * Colours the whole app with a heavily blurred gradient drawn from the current photo.
 * Lightness comes from the active theme so text contrast is preserved.
 */
export function usePhotoPalette() {
  const selectedPath = useEditorStore((s) => s.selectedImage?.path ?? null);
  const activePath = useLibraryStore((s) => s.libraryActivePath);
  const firstSelected = useLibraryStore((s) => s.multiSelectedPaths[0] ?? null);
  const path = selectedPath ?? activePath ?? firstSelected;
  const thumbnail = useProcessStore((s) => (path ? s.thumbnails[path] : undefined));
  const lastUrl = useRef<string | null>(null);

  useEffect(() => {
    if (!thumbnail || thumbnail === lastUrl.current) return;
    lastUrl.current = thumbnail;
    let cancelled = false;
    const img = new Image();
    img.onload = () => {
      if (cancelled) return;
      const palette = extractPalette(img);
      if (!palette) return;
      const root = document.documentElement.style;
      palette.hues.forEach((h, i) => {
        const l = 'calc(var(--app-photo-lightness, 0.3) * 100%)';
        root.setProperty(`--photo-c${i + 1}`, `hsl(${h.toFixed(0)} ${(palette.sats[i] * 100).toFixed(0)}% ${l})`);
      });
    };
    img.src = thumbnail;
    return () => {
      cancelled = true;
    };
  }, [thumbnail]);
}
