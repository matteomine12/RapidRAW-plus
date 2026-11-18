# Smart LUT — design, state and handoff

Smart LUT is a RapidRAW+ feature: a *look* extracted from a reference photo, one colour or a palette, saved,
and applied to any photo **under** the user's edit (every other adjustment stays on top). It adapts to each
photo: tones are relative to the photo's own median, colour is transferred per tonal band, and skin, sky and
vegetation get their own treatment from AI masks. Two amounts are exposed, **colour** and **luminance**, on a
square pad inspired by Apple's Photographic Styles.

This document is for whoever continues the work (human or agent). Start here, then read the files listed.

## Status (2026-11-18)

| Phase | What | State |
| --- | --- | --- |
| 1 | Look format, storage, per-photo LUT compiler, shader stage | done, tested |
| 2 | Analytic extraction from a photo / colour / palette | done, tested on real photos (CPU simulation) |
| 3 | UI: library, creation, colour × luminance pad | done, typechecked; needs hands-on testing in the app |
| 4 | Trained model (neural engine) to compare with the analytic one | scripts ready in `training/smart_lut/`, **not run yet** (no PyTorch / dataset downloaded) |

Work lives on branch `smart-lut` (not merged into `main`).

## User requirements (from the owner, in Italian originally)

- A look can be made from a photo, a colour, or a set of colours; looks are saved.
- Applying a look edits the photo by itself: colours (can change completely), luminance of highlights and
  shadows, contrast — differently for sky, skin, details…, based on what is extracted.
- Must be smart: a black-and-white reference makes a B&W look with that photo's contrast; an underexposed
  reference makes a dark look; a less saturated reference makes a less saturated look.
- It works **under** the user's edits (edits are applied on top of the look).
- AI models must be downloaded to use the feature (no global-only fallback).
- Pad: bottom left = no effect (neutral), up = luminance, right = colour; **100% at the top right corner**
  (0..100 on both axes). Colours of the pad come from the look.
- Owner's workflow: commit and push after each chunk; git identity
  `-c user.name=matteomine12 -c user.email=matteomine12@users.noreply.github.com`.

## How it works

### Data flow

1. **Create a look** (`create_smart_lut_from_image` / `create_smart_lut_from_colors`): the reference is
   analysed (`stats::analyze`) together with skin/sky/vegetation masks from the existing AI models
   (`ai::commands::smart_lut_region_masks`: ESAM3 sky prompt, ESAM3 text prompt "vegetation", face analysis
   skin parts). `look::from_reference` / `look::from_colors` turn that into a `SmartLook` JSON saved in
   `<app data>/smart_luts/<id>.json`.
2. **Apply** (frontend `useSmartLut().applyLook`): the edit gets
   `smartLut: { look, color: 100, luminance: 100, analysis }`.
3. **Analyse the open photo** (`analyze_smart_lut_target`, run automatically by `useSmartLutAutoAnalysis`
   whenever `analysis.path` differs from the open photo): statistics of the photo plus its region masks,
   stored as `smartLut.analysis = { id, path, stats, regions: { skin|sky|vegetation: { maskDataBase64,
   rotation, flipHorizontal, flipVertical, orientationSteps } | null } }` (masks in the same form as AI
   sub-masks).
4. **Render** (every render path: editor preview, export, thumbnails, previews): `smart_lut::render::attach`
   compiles the look against the analysis (cached), appends the region masks as extra layers after the user's
   mask layers, and fills the uniforms. The shader applies it.

### The LUTs (`compile.rs`)

Four 33³ RGB tables stacked along blue (slot 0 whole image, 1 skin, 2 sky, 3 vegetation), uploaded as one
3D texture (binding 16, `smart_lut_texture`). Domain: **input** = V-Log of the photo's scene light normalised
so its median luma is 0.18; **output** = V-Log of the scene light the display transform should receive.

Per entry: tone curve on luma (log space, monotone cubic, slopes bounded), then colour in OKLab chroma
relative to lightness `c = (a/L, b/L)`, per tonal band (Bernstein weights over stops from the median):
`c' = ref_mean + (c − target_mean) · s`, with `s` = ratio of chroma spreads (0 for B&W looks), the photo's
own cast removed only up to 0.08, an optional pull of hues toward the look's palette, whites/blacks spared
from the tint, and skin kept 65% to its own colour when the reference had no people.

Tone transfer for photo looks: target quantiles (scene, relative to median) → the reference's on-screen luma
quantiles, mapped back through the inverse of the shader's display transform (`color::display_inverse`, a CPU
copy of basic/AgX), blended 80% with a standard rendering. Colour looks use a parametric curve (key shift,
contrast).

### Shader (`src-tauri/src/shaders/shader.wgsl`, `apply_smart_lut`)

Runs just before exposure: `n = src · 0.18 / 2^key` (key = median log2 luma already computed by the tone
base pass, `airlight_texture (2,0).w`), sample slot 0, blend the region slots by their mask layers
(`smart_region_start`, `smart_regions` bits). Then mix with the default render (`src · 2^baseline`) in OKLab:
lightness by `smart_luminance`, chroma-over-lightness by `smart_color`. The result replaces the baseline
exposure; the user's exposure (and masks') still multiplies on top. Feature flag `F_SMART_LUT = 4096`.
Uniform fields reuse former padding: `smart_baseline`, `smart_lut_enabled`, `smart_color`, `smart_luminance`,
`smart_regions`, `smart_region_start`.

### Frontend

- `src/components/adjustments/SmartLut.tsx` — section panel: pad, library tiles, create from photo / colours,
  rename and delete (context menu).
- `src/components/adjustments/SmartLutPad.tsx` — the square: 11×11 dots, lit row/column through the handle,
  ring at bottom left = no effect, drag / arrow keys / double-click (back to 100/100).
- `src/hooks/useSmartLut.ts` — apply/clear, analysis, auto re-analysis (mounted in `ControlsPanel`).
- `src/utils/smartLut.ts` — types, pad gradient (`lookGradient`).
- Region masks are sent to the backend once, then as `null` (`backendPayload.ts`; restored in
  `render/adjustment_utils.rs::hydrate_smart_lut_regions`).
- New section `smartLut` in `ADJUSTMENT_SECTIONS`, copy/paste group, i18n in `en.json` / `it.json`.

## Files

```
src-tauri/src/adjustments/smart_lut/
  mod.rs      Tauri commands (list/create/rename/delete/analyse), reference loading, mask softening
  color.rs    sRGB, OKLab, V-Log, CPU copy of the display transform and its inverse
  stats.rs    image statistics (quantiles, band colour stats, regions, palette k-means)
  look.rs     SmartLook format; from_reference, from_colors
  compile.rs  look + photo stats → 4 LUTs; tests; `visual_check` (CPU render for eyeballing / evaluation)
  render.rs   attach(): compile cache, region mask layers, uniforms
src-tauri/src/ai/commands.rs        smart_lut_region_masks()
src-tauri/src/render/gpu/processor.rs   binding 16 texture, RenderRequest.smart_lut, GPU tests (smart_lut_tests)
training/smart_lut/                 phase 4 (see its README)
```

## Testing

```bash
cd src-tauri && cargo test --lib smart_lut          # unit + GPU render tests
SMART_REF=ref.jpg SMART_TARGET=photo.jpg SMART_OUT=/tmp/out.png \
  cargo test --lib visual_check -- --ignored        # CPU render of a look on a JPEG (ref may be "#rrggbb,#rrggbb")
npx tsc --noEmit                                    # frontend
npm run start                                       # the app (tauri dev)
```

GPU tests wait for the background pipeline specialisation to finish; without that the process segfaults at
exit (driver tearing down mid-compile), not a Smart LUT bug.

## Known limits / ideas

- Pasting a look onto photos without opening them: renders (export, thumbnails) use the source photo's
  analysis until the photo is opened and re-analysed. A fix would pass the photo path to `attach` and skip
  regions (or stats) when `analysis.path` differs, or analyse on paste.
- Skin comes from the face models only (skin near detected faces).
- Colour looks tint fairly strongly; tune `look::from_colors` (band scales, saturation, hue pull).
- Only the basic/AgX display transforms are mirrored on CPU; changes to `display_transform` in the shader must
  be mirrored in `color::display_gray`.

## Phase 4 — next steps

1. `cd training/smart_lut && ./setup.sh && ./download_data.sh` (the owner approved downloading these for the
   training session).
2. `.venv/bin/python train.py --data data --iters 60000 --name v1`, then `evaluate.py --name v1` (scored
   analytic vs neural) and `evaluate.py --name v1 --pairs ref.jpg:photo.jpg` (real looks).
3. If the neural engine wins: `export_onnx.py`, host the ONNX like the other models
   (`src-tauri/src/ai/models.rs`), store a reference thumbnail with photo looks, and convert the model's
   sRGB→sRGB LUT into the engine's domain (scene → `display_gray` → neural LUT → `display_inverse`). Add an
   "analytic / neural" choice in the panel so the owner can compare. Colour/palette looks stay analytic (no
   reference photo). The training README has the details.
