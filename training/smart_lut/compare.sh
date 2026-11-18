#!/usr/bin/env bash
# Try a reference look on your own photos with both engines, side by side, and open the result.
#   ./compare.sh riferimento.jpg foto1.jpg [foto2.jpg …]
# Columns: reference | photo | analytic (the app's engine) | neural (model v1). JPEG/PNG/TIFF only.
set -euo pipefail
cd "$(dirname "$0")"
[ $# -ge 2 ] || { echo "uso: ./compare.sh riferimento.jpg foto1.jpg [foto2.jpg …]"; exit 1; }
ref=$(realpath "$1"); shift
pairs=()
for photo in "$@"; do pairs+=("$ref:$(realpath "$photo")"); done
.venv/bin/python evaluate.py --name "${MODEL:-v1}" --pairs "${pairs[@]}"
xdg-open "eval/${MODEL:-v1}/real_pairs.jpg" >/dev/null 2>&1 &
