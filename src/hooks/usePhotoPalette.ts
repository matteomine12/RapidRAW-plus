import { useEffect, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Invokes } from '../types/invokes';
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
function extractPalette(img: CanvasImageSource): Palette | null {
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

  // Exposure-neutral: scale every pixel so the image averages mid-grey. Hue and saturation then no longer
  // depend on how bright or dark the current edit (or camera) exposure makes the picture.
  let lumaSum = 0;
  for (let i = 0; i < data.length; i += 4) {
    lumaSum += 0.2126 * data[i] + 0.7152 * data[i + 1] + 0.0722 * data[i + 2];
  }
  const meanLuma = lumaSum / (data.length / 4) / 255;
  const gain = Math.max(0.35, Math.min(6, 0.42 / Math.max(meanLuma, 0.001)));

  const weight = new Array<number>(HUE_BINS).fill(0);
  const hueSum = new Array<number>(HUE_BINS).fill(0);
  const satSum = new Array<number>(HUE_BINS).fill(0);
  let totalSat = 0;

  for (let i = 0; i < data.length; i += 4) {
    const [h, s, l] = rgbToHsl(
      Math.min(255, data[i] * gain),
      Math.min(255, data[i + 1] * gain),
      Math.min(255, data[i + 2] * gain),
    );
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
 * Thumbnails are served from Tauri's asset protocol, which is cross-origin for the webview, so drawing an
 * <img> would taint the canvas. Fetching the bytes and decoding them into an ImageBitmap keeps pixels readable.
 */
async function loadSource(url: string): Promise<CanvasImageSource> {
  try {
    const res = await fetch(url);
    if (res.ok) return await createImageBitmap(await res.blob());
  } catch {
    // fall through to the <img> path
  }
  return await new Promise<HTMLImageElement>((resolve, reject) => {
    const img = new Image();
    img.crossOrigin = 'anonymous';
    img.onload = () => resolve(img);
    img.onerror = reject;
    img.src = url;
  });
}

/**
 * Colours the whole app with a heavily blurred gradient drawn from the current photo.
 * Lightness comes from the active theme so text contrast is preserved.
 */
export function usePhotoPalette() {
  const selectedPath = useEditorStore((s) => s.selectedImage?.path ?? null);
  const isReady = useEditorStore((s) => s.selectedImage?.isReady ?? false);
  const adjustments = useEditorStore((s) => s.adjustments);
  const isSliderDragging = useEditorStore((s) => s.isSliderDragging);
  const activePath = useLibraryStore((s) => s.libraryActivePath);
  const firstSelected = useLibraryStore((s) => s.multiSelectedPaths[0] ?? null);
  const path = selectedPath ?? activePath ?? firstSelected;
  const thumbnail = useProcessStore((s) => (path ? s.thumbnails[path] : undefined));
  const theme = useSettingsStore((s) => s.theme);
  const hasPhoto = useRef(false);
  const renderSeq = useRef(0);

  const paintPalette = (palette: Palette) => {
    const lum = parseFloat(
      THEMES.find((t) => t.id === (theme || DEFAULT_THEME_ID))?.cssVariables['--app-photo-luminance'] ?? '0.1',
    );
    const root = document.documentElement.style;
    palette.hues.forEach((h, i) => {
      const l = lightnessForLuminance(h, palette.sats[i], lum);
      root.setProperty(
        `--photo-c${i + 1}`,
        `hsl(${h.toFixed(0)} ${(palette.sats[i] * 100).toFixed(0)}% ${(l * 100).toFixed(1)}%)`,
      );
    });
  };

  // Library / quick colour: read the cached thumbnail.
  useEffect(() => {
    if (!thumbnail) {
      // Nothing to colour from yet: a neutral, colourless wash at the theme's luminance.
      if (!hasPhoto.current) paintPalette({ hues: [0, 0, 0], sats: [0, 0, 0] });
      return;
    }
    let cancelled = false;
    loadSource(thumbnail)
      .then((source) => {
        if (cancelled) return;
        const palette = extractPalette(source);
        if (!palette) {
          console.warn('[photo-palette] could not read pixels from', thumbnail);
          return;
        }
        hasPhoto.current = true;
        paintPalette(palette);
      })
      .catch((err) => console.warn('[photo-palette] failed to load', thumbnail, err));
    return () => {
      cancelled = true;
    };
  }, [thumbnail, theme]);

  // Editor: render the edited photo with exposure forced to 0 and HDR off (so it is plain SDR), then colour from that.
  useEffect(() => {
    if (!selectedPath || !isReady || isSliderDragging) return;
    const seq = ++renderSeq.current;
    const timer = setTimeout(async () => {
      try {
        const bytes = await invoke<Uint8Array>(Invokes.GeneratePreviewForPath, {
          path: selectedPath,
          jsAdjustments: { ...adjustments, exposure: 0, hdrEnabled: false },
        });
        if (seq !== renderSeq.current) return;
        const bitmap = await createImageBitmap(new Blob([new Uint8Array(bytes)], { type: 'image/jpeg' }));
        if (seq !== renderSeq.current) return;
        const palette = extractPalette(bitmap);
        bitmap.close();
        if (palette) {
          hasPhoto.current = true;
          paintPalette(palette);
        }
      } catch (err) {
        console.warn('[photo-palette] exposure-neutral render failed', err);
      }
    }, 800);
    return () => clearTimeout(timer);
  }, [adjustments, selectedPath, isReady, isSliderDragging, theme]);
}
