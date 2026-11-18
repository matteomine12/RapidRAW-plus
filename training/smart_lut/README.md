# Smart LUT model (phase 4)

A small network that looks at a reference photo and the photo being edited and returns a 33³ 3D LUT that
puts the reference's look on the photo. It is trained without labels: random looks (tones relative to the
photo's median, colour per tonal band, black and white, toning, per-hue shifts — see `smartlut/styles.py`)
are put on photo A and photo B; the model sees A-with-the-look and plain B and must produce B-with-the-look.
The output has the same layout as the app's `SmartLutData`, so it can replace or complement the analytic
engine (`src-tauri/src/adjustments/smart_lut/`).

## Steps

```bash
cd training/smart_lut
./setup.sh                 # .venv with Python 3.12 + PyTorch CUDA (~3 GB download)
./download_data.sh         # COCO 2017 val, 5k photos (~1 GB); `./download_data.sh train` adds 118k (~18 GB)
.venv/bin/python train.py --data data --iters 60000 --name v1      # ~1 h on an RTX 3080
.venv/bin/python evaluate.py --name v1                             # analytic vs neural, scored
.venv/bin/python evaluate.py --name v1 --pairs ref.jpg:photo.jpg   # real references, side by side
.venv/bin/python export_onnx.py --name v1                          # runs/v1/smart_lut_v1.onnx
```

`train.py --resume` continues an interrupted run. Checkpoints, logs and evaluations go to `runs/` and
`eval/` (not committed).

## Reading the evaluation

`evaluate.py` gives each held-out pair a random look and scores both engines by the mean OKLab distance (×100)
from the true result: `identity` is doing nothing, `analytic` is the app's current engine (run through its
own Rust code, `visual_check`), `neural` is the model. `neural_better_on` is the share of pairs where the
model wins. `eval/<name>/heldout.jpg` shows reference | target | answer | analytic | neural.

The random looks are the analytic engine's own vocabulary, so the score favours neither by design, but real
references are the real test: run `--pairs` on photos whose look you like.

## Better data

MIT-Adobe FiveK (5,000 RAWs, each retouched by five experts) gives real styles instead of random ones:
download from https://data.csail.mit.edu/graphics/fivek/ (about 50 GB), export the expert TIFFs as JPEG into
`data/fivek/<expert>/`, and add a pair source that uses expert X's edit of photo A as the reference and
expert X's edit of photo B as the answer.

## Next in the app

Load the ONNX with `ort` (as the other models in `src-tauri/src/ai/models.rs`), run it when a look is applied
(reference thumbnail stored with the look, target = the open photo's preview), and convert its sRGB→sRGB LUT
into the engine's domain: for each grid entry, scene light → display (`display_gray` per channel) → neural
LUT → `display_inverse`. Offer "analytic / neural" as an option to compare in the editor.
