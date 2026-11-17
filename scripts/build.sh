#!/usr/bin/env bash
# Build RapidRAW+ for Linux x86_64 on Fedora (native, or inside a Fedora container).
# Usage:  ./scripts/build.sh            (env: RR_FEATURES="tethering", RR_DEBUG=1)
set -euo pipefail

cd "$(dirname "$0")/.."

if [ "$(uname -m)" != "x86_64" ]; then
  echo "Questo script produce build x86_64: host rilevato $(uname -m)." >&2
  exit 1
fi

SUDO=""
[ "$(id -u)" -ne 0 ] && SUDO="sudo"

echo ">> Installo dipendenze di sistema (dnf)..."
$SUDO dnf install -y \
  webkit2gtk4.1-devel openssl-devel curl wget file gcc gcc-c++ make \
  libappindicator-gtk3-devel librsvg2-devel libxdo-devel pkgconf-pkg-config \
  rpm-build patchelf git nodejs npm \
  ${RR_FEATURES:+libgphoto2-devel}

if ! command -v rustup >/dev/null 2>&1; then
  echo ">> Installo rustup..."
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
fi
# shellcheck disable=SC1091
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"

# rust-toolchain.toml in src-tauri fissa la versione di Rust usata dal progetto
echo ">> npm install"
npm install

ARGS=(tauri build)
[ "${RR_DEBUG:-0}" = "1" ] && ARGS+=(--debug)
[ -n "${RR_FEATURES:-}" ] && ARGS+=(--features "$RR_FEATURES")

# NO_STRIP: necessario per la creazione dell'AppImage (come nella CI upstream)
echo ">> npx ${ARGS[*]}"
NO_STRIP=true npx "${ARGS[@]}"

PROFILE=release; [ "${RR_DEBUG:-0}" = "1" ] && PROFILE=debug
echo
echo "Fatto. Pacchetti in: src-tauri/target/$PROFILE/bundle"
