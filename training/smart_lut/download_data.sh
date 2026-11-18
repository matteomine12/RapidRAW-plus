#!/usr/bin/env bash
# Training photos. Default: COCO 2017 val (5k photos, ~1 GB).
#   ./download_data.sh            COCO val (~1 GB)
#   ./download_data.sh train      also COCO train (118k photos, ~18 GB) for a stronger model
# MIT-Adobe FiveK (expert retouches, best for judging quality) needs a manual download: see README.
set -euo pipefail
cd "$(dirname "$0")"
mkdir -p data
fetch() {
  local name=$1
  if [ -d "data/$name" ]; then echo "data/$name already there"; return; fi
  curl -L --fail -o "data/$name.zip" "http://images.cocodataset.org/zips/$name.zip"
  (cd data && unzip -q "$name.zip" && rm "$name.zip")
}
fetch val2017
if [ "${1:-}" = "train" ]; then fetch train2017; fi
echo "Photos: $(find data -iname '*.jpg' | wc -l)"
