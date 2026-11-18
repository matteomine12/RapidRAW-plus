export interface SmartLutPaletteColor {
  /** Display sRGB, 0..1. */
  rgb: [number, number, number];
  weight: number;
}

export type SmartLutEngine = 'analytic' | 'neural';

export type SmartLookKind = 'image' | 'color' | 'palette';

/** A saved Smart LUT recipe (see src-tauri/src/adjustments/smart_lut/look.rs). */
export interface SmartLook {
  version: number;
  id: string;
  name: string;
  kind: SmartLookKind;
  createdAt: number;
  palette: Array<SmartLutPaletteColor>;
  monochrome: boolean;
  tone: unknown;
  bands: unknown;
  saturation: number | null;
  regions: unknown;
  huePull: number;
  /** A reference thumbnail is saved with the look (needed by the neural engine). */
  hasReference?: boolean;
}

export interface SmartLutRegionMask {
  maskDataBase64: string | null;
  rotation: number;
  flipHorizontal: boolean;
  flipVertical: boolean;
  orientationSteps: number;
}

export type SmartLutRegionName = 'skin' | 'sky' | 'vegetation';

/** What the open photo looks like to the Smart LUT: its statistics and region masks. */
export interface SmartLutAnalysis {
  id: string;
  /** The photo it was made for; another photo needs a new analysis. */
  path?: string;
  stats: unknown;
  regions: Partial<Record<SmartLutRegionName, SmartLutRegionMask | null>>;
}

export interface SmartLutState {
  look: SmartLook;
  /** 0..100, 100 = the look as extracted. */
  color: number;
  /** 0..100, 100 = the look as extracted. */
  luminance: number;
  analysis: SmartLutAnalysis | null;
  /** Which engine builds the LUTs; the neural one is the trained model (beta), for comparison. */
  engine?: SmartLutEngine;
  /** Bumped when the neural engine has been run for this photo, so the preview re-renders. */
  neuralStamp?: number;
}

export const SMART_LUT_REGIONS: Array<SmartLutRegionName> = ['skin', 'sky', 'vegetation'];
export const SMART_LUT_NEUTRAL = 100;
export const SMART_LUT_MAX = 100;

export const smartLutRegionCacheId = (analysisId: string, region: string) => `smartlut:${analysisId}:${region}`;

type Rgb = [number, number, number];

const luminance = ([r, g, b]: Rgb) => 0.2126 * r + 0.7152 * g + 0.0722 * b;
const chroma = ([r, g, b]: Rgb) => Math.max(r, g, b) - Math.min(r, g, b);
const mix = (a: Rgb, b: Rgb, t: number): Rgb => [
  a[0] + (b[0] - a[0]) * t,
  a[1] + (b[1] - a[1]) * t,
  a[2] + (b[2] - a[2]) * t,
];
const css = (c: Rgb, alpha = 1) =>
  `rgba(${Math.round(c[0] * 255)}, ${Math.round(c[1] * 255)}, ${Math.round(c[2] * 255)}, ${alpha})`;

/** The colours a look's square is painted with, from its palette. */
export function lookSwatch(look: Pick<SmartLook, 'palette' | 'monochrome'>): {
  accent: Rgb;
  second: Rgb;
  base: Rgb;
  dark: Rgb;
} {
  const colors = (look.palette ?? []).map((p) => p.rgb as Rgb);
  if (colors.length === 0 || look.monochrome) {
    const grey = (v: number): Rgb => [v, v, v];
    return { accent: grey(0.78), second: grey(0.55), base: grey(0.32), dark: grey(0.05) };
  }
  const byChroma = [...colors].sort((a, b) => chroma(b) - chroma(a));
  const byLight = [...colors].sort((a, b) => luminance(a) - luminance(b));
  const accent = byChroma[0];
  const second = byChroma[1] ?? byLight[byLight.length - 1];
  const base = mix(byLight[Math.floor(byLight.length / 2)], [0.12, 0.12, 0.12], 0.45);
  const dark = mix(byLight[0], [0, 0, 0], 0.75);
  return { accent, second, base, dark };
}

/**
 * CSS background for a look's square, read like the pad: bottom left is no effect (a neutral light grey),
 * going right brings in the look's colours, going up its tones (a grey as light or dark as the look's
 * shadows and midtones), and the top right corner is the whole look.
 */
export function lookGradient(look: Pick<SmartLook, 'palette' | 'monochrome'>): string {
  const { accent, second, base, dark } = lookSwatch(look);
  const neutral: Rgb = [0.86, 0.86, 0.85];
  const tone = luminance(mix(base, dark, 0.5));
  const grey: Rgb = [tone, tone, tone];
  return [
    `linear-gradient(0deg, ${css(grey, 0)} 0%, ${css(grey, 0.5)} 55%, ${css(grey, 0.88)} 100%)`,
    `linear-gradient(90deg, ${css(neutral)} 0%, ${css(mix(neutral, second, 0.75))} 50%, ${css(accent)} 100%)`,
  ].join(', ');
}

export const hexToRgb = (hex: string): Rgb => {
  const v = parseInt(hex.replace('#', ''), 16);
  return [((v >> 16) & 255) / 255, ((v >> 8) & 255) / 255, (v & 255) / 255];
};

/** Whether the look is on but has not been read for this photo yet. */
export const needsAnalysis = (state: SmartLutState | null | undefined, path: string | undefined): boolean =>
  !!state && !!path && (!state.analysis || state.analysis.path !== path);
