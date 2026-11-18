#!/usr/bin/env bash
# Creates .venv with Python 3.12 (PyTorch has no wheels for the system's Python 3.15) and installs
# PyTorch with CUDA plus the few libraries the scripts use. Downloads ~3 GB.
set -euo pipefail
cd "$(dirname "$0")"

if ! command -v uv >/dev/null 2>&1; then
  echo "Installing uv (Python package/venv manager) for this user…"
  curl -LsSf https://astral.sh/uv/install.sh | sh
  export PATH="$HOME/.local/bin:$PATH"
fi

uv venv --python 3.12 .venv
uv pip install --python .venv/bin/python torch torchvision --index-url https://download.pytorch.org/whl/cu124
uv pip install --python .venv/bin/python numpy pillow tqdm onnx onnxruntime

.venv/bin/python - <<'PY'
import torch
print("torch", torch.__version__, "CUDA:", torch.cuda.is_available(),
      torch.cuda.get_device_name(0) if torch.cuda.is_available() else "")
PY
