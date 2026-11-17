import { useEffect, useRef } from 'react';
import { useEditorStore } from '../store/useEditorStore';
import { useLibraryStore } from '../store/useLibraryStore';
import { useProcessStore } from '../store/useProcessStore';
import { useSettingsStore } from '../store/useSettingsStore';
import { DEFAULT_THEME_ID, THEMES } from '../utils/themes';

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

function hslToRgb(h: number, s: number, l: number): [number, number, number] {
  const k = (n: number) => (n + h / 30) % 12;
  const a = s * Math.min(l, 1 - l);
  const f = (n: number) => l - a * Math.max(-1, Math.min(k(n) - 3, Math.min(9 - k(n), 1)));
  return [f(0), f(8), f(4)];
}

function luminance([r, g, b]: [number, number, number]): number {
  const lin = (c: number) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}

/** Finds the HSL lightness whose relative luminance matches the target, so text contrast is guaranteed without a scrim. */
function lightnessForLuminance(h: number, s: number, target: number): number {
  let lo = 0.03;
  let hi = 0.97;
  for (let i = 0; i < 14; i++) {
    const mid = (lo + hi) / 2;
    if (luminance(hslToRgb(h, s, mid)) < target) lo = mid;
    else hi = mid;
  }
  return (lo + hi) / 2;
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
  const cap = meanSat < 0.08 ? 0.12 : 0.85;
  const sat = (s: number) => Math.min(cap, Math.max(cap === 0.12 ? 0.04 : 0.55, s));
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
  const theme = useSettingsStore((s) => s.theme);
  const lastKey = useRef<string | null>(null);

  useEffect(() => {
    const target = parseFloat(
      THEMES.find((t) => t.id === (theme || DEFAULT_THEME_ID))?.cssVariables['--app-photo-luminance'] ?? '0.1',
    );
    const paint = (hues: number[], sats: number[]) => {
      const root = document.documentElement.style;
      hues.forEach((h, i) => {
        const l = lightnessForLuminance(h, sats[i], target);
        root.setProperty(
          `--photo-c${i + 1}`,
          `hsl(${h.toFixed(0)} ${(sats[i] * 100).toFixed(0)}% ${(l * 100).toFixed(1)}%)`,
        );
      });
    };
    if (!thumbnail) {
      if (lastKey.current === null) paint([230, 310, 190], [0.7, 0.6, 0.65]);
      return;
    }
    const key = `${theme}|${thumbnail}`;
    if (key === lastKey.current) return;
    lastKey.current = key;
    let cancelled = false;
    const img = new Image();
    img.onload = () => {
      if (cancelled) return;
      const palette = extractPalette(img);
      if (palette) paint(palette.hues, palette.sats);
    };
    img.src = thumbnail;
    return () => {
      cancelled = true;
    };
  }, [thumbnail, theme]);
}
