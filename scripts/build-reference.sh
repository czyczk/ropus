#!/usr/bin/env bash
# Build the pinned reference toolchain used by validation.
#
# Produces, under ${ROPUS_REFBUILD:-.refbuild}:
#   opus-src/        libopus v1.6.1 default float build + opus_demo
#   opus-src-purec/  libopus v1.6.1 scalar float build + opus_demo
#   opus-src-fixed16/ libopus v1.6.1 fixed-point 16-bit build + opus_demo
#   prefix-tools/    opusdec (opus-tools v0.2) linked against prefix-prod
#
# All ML paths (dred, deep-plc, osce) are disabled. The fixed-point 16-bit
# build is the primary bit-exact oracle for the fixed-point Rust core.
set -euo pipefail

ROOT="${ROPUS_REFBUILD:-$(cd "$(dirname "$0")/.." && pwd)/.refbuild}"
REF="${OPUS_REF_SOURCE:-$HOME/src/public/opus}"
PINSRC_COMMIT="22244de5a79bd1d6d623c32e72bf1954b56235be"

mkdir -p "$ROOT"
cd "$ROOT"

clone_at() {
  local dir=$1
  if [ ! -d "$dir/.git" ]; then
    git clone -q "$REF" "$dir"
  fi
  git -C "$dir" checkout -q "$PINSRC_COMMIT"
}

build_libopus() {
  local dir=$1 prefix=$2
  shift 2
  clone_at "$dir"
  (
    cd "$dir"
    [ -x ./configure ] || ./autogen.sh
    ./configure --prefix="$ROOT/$prefix" "$@"
    make -j"$(nproc)"
    make install
  )
}

echo "== libopus v1.6.1 production (float) =="
build_libopus opus-src prefix-prod \
  --disable-dred --disable-deep-plc --disable-osce --enable-extra-programs

echo "== libopus v1.6.1 pure C (float) =="
build_libopus opus-src-purec prefix-purec \
  --disable-dred --disable-deep-plc --disable-osce \
  --disable-asm --disable-intrinsics --disable-rtcd --enable-extra-programs

echo "== libopus v1.6.1 fixed-point 16-bit (primary oracle) =="
build_libopus opus-src-fixed16 prefix-fixed16 \
  --enable-fixed-point --disable-fixed-res24 \
  --disable-dred --disable-deep-plc --disable-osce \
  --disable-asm --disable-intrinsics --disable-rtcd --enable-extra-programs

echo "== opusfile =="
if [ ! -d opusfile/.git ]; then
  git clone -q --depth 1 --branch v0.12 https://github.com/xiph/opusfile.git opusfile
fi
(
  cd opusfile
  [ -x ./configure ] || ./autogen.sh
  PKG_CONFIG_PATH="$ROOT/prefix-prod/lib/pkgconfig" ./configure --prefix="$ROOT/prefix-opusfile"
  make -j"$(nproc)"
  make install
)

echo "== libopusenc =="
if [ ! -d libopusenc/.git ]; then
  git clone -q --depth 1 --branch v0.2.1 https://github.com/xiph/libopusenc.git libopusenc
fi
(
  cd libopusenc
  [ -x ./configure ] || ./autogen.sh
  PKG_CONFIG_PATH="$ROOT/prefix-prod/lib/pkgconfig" ./configure --prefix="$ROOT/prefix-libopusenc"
  make -j"$(nproc)"
  make install
)

echo "== opus-tools v0.2 (opusdec) =="
if [ ! -d opus-tools/.git ]; then
  git clone -q --depth 1 --branch v0.2 https://github.com/xiph/opus-tools.git opus-tools
fi
(
  cd opus-tools
  [ -x ./configure ] || ./autogen.sh
  PKG_CONFIG_PATH="$ROOT/prefix-prod/lib/pkgconfig:$ROOT/prefix-opusfile/lib/pkgconfig:$ROOT/prefix-libopusenc/lib/pkgconfig" \
    ./configure --with-opus="$ROOT/prefix-prod" \
                --with-opusfile="$ROOT/prefix-opusfile" \
                --with-libopusenc="$ROOT/prefix-libopusenc" \
                --prefix="$ROOT/prefix-tools"
  # opusenc's vendored compatibility shim fails under -Werror with the pinned
  # libopusenc headers; opusdec is the only tool needed for validation.
  make opusdec
  cp opusdec "$ROOT/prefix-tools/bin/opusdec" 2>/dev/null || {
    mkdir -p "$ROOT/prefix-tools/bin"
    cp opusdec "$ROOT/prefix-tools/bin/opusdec"
  }
)

echo "Reference toolchain ready under $ROOT"
"$ROOT/opus-tools/opusdec" --version | head -n 1
