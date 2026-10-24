#!/bin/bash
# Patched-libwebrtc build for the AV1-E2EE fix. Derived from build_linux.sh at
# tag webrtc-51ef663, with two deliberate differences: patches are NOT applied
# here (they are applied once, by hand, so this script is idempotent and can
# be re-run while iterating), and the cryptor object is compiled FIRST so a
# syntax error in the patch fails in seconds, not hours into the build.
set -e
cd "$(dirname "$0")"
export PATH="$PWD/depot_tools:$PATH"

arch=x64
profile=release
OUTPUT_DIR="$PWD/src/out-$arch-$profile"
# WEBRTC_ARTIFACTS_DIR writes the archive beside an existing one instead of over it.
ARTIFACTS_DIR="${WEBRTC_ARTIFACTS_DIR:-$PWD/linux-$arch-$profile}"
case "$ARTIFACTS_DIR" in
  /*) ;;
  *) echo "FATAL: WEBRTC_ARTIFACTS_DIR must be an absolute path" >&2; exit 1 ;;
esac
# Its include/ is deleted and rebuilt below, so it must be a directory of its own: not
# inside src/ or depot_tools/, and not this directory or one of its parents.
ROOT=$(pwd -P)
bad_dir=0
ARTIFACTS_DIR=$(realpath -m -- "$ARTIFACTS_DIR")
case "$ARTIFACTS_DIR/" in
  "$ROOT"/src/*|"$ROOT"/depot_tools/*) bad_dir=1 ;;
esac
art_prefix=${ARTIFACTS_DIR%/}
case "$ROOT/" in
  "$art_prefix"/*) bad_dir=1 ;;
esac
if [ "$bad_dir" = 1 ]; then
  echo "FATAL: artifact directory $ARTIFACTS_DIR overlaps the recipe tree" >&2
  exit 1
fi

python3 "./src/build/linux/sysroot_scripts/install-sysroot.py" --arch="$arch"

# args verbatim from build_linux.sh (= the prebuilt's args.gn)
args="is_debug=false  \
  target_os=\"linux\" \
  target_cpu=\"$arch\" \
  rtc_enable_protobuf=false \
  treat_warnings_as_errors=false \
  use_llvm_libatomic=false \
  use_custom_libcxx=false \
  use_custom_libcxx_for_host=false \
  use_clang_modules=false \
  rtc_include_tests=false \
  rtc_build_tools=false \
  rtc_build_examples=false \
  rtc_libvpx_build_vp9=true \
  enable_libaom=true \
  is_component_build=false \
  enable_stripping=true \
  ffmpeg_branding=\"Chrome\" \
  rtc_use_h264=true \
  rtc_use_h265=true \
  rtc_use_pipewire=true \
  symbol_level=0 \
  enable_iterator_debugging=false \
  use_rtti=true \
  rtc_use_x11=true"

gn gen "$OUTPUT_DIR" --root="src" --args="${args}"

# Fail-fast gate: compile the patched file alone before the multi-hour build.
cryptor_obj=$(ninja -C "$OUTPUT_DIR" -t targets all | grep -m1 'frame_crypto_transformer\.o' | cut -d: -f1)
if [ -z "$cryptor_obj" ]; then
  echo "FATAL: no ninja target for frame_crypto_transformer.o" >&2
  exit 1
fi
echo "=== fail-fast: building $cryptor_obj ==="
ninja -C "$OUTPUT_DIR" "$cryptor_obj"

dav1d_obj=$(ninja -C "$OUTPUT_DIR" -t targets all | grep -m1 'av1/dav1d_decoder/dav1d_decoder\.o' | cut -d: -f1)
if [ -z "$dav1d_obj" ]; then
  echo "FATAL: no ninja target for dav1d_decoder.o" >&2
  exit 1
fi
echo "=== fail-fast: building $dav1d_obj ==="
ninja -C "$OUTPUT_DIR" "$dav1d_obj"

echo "=== full build ==="
ninja -C "$OUTPUT_DIR" :default

echo "=== assembling artifacts ==="
mkdir -p "$ARTIFACTS_DIR/lib"
rm -f "$ARTIFACTS_DIR/lib/libwebrtc.a"
ar -rc "$ARTIFACTS_DIR/lib/libwebrtc.a" `find "$OUTPUT_DIR/obj" -name '*.o' -not -path "*/third_party/nasm/*"`
src/third_party/llvm-build/Release+Asserts/bin/llvm-objcopy --redefine-syms="$PWD/boringssl_prefix_symbols.txt" "$ARTIFACTS_DIR/lib/libwebrtc.a"

# Both patches carry a grep-able stamp; an archive without them was built from
# an unpatched (or older) tree and must never be linked.
for stamp in tinhat-av1-e2ee-v2 tinhat-dav1d-fp-v1; do
  if ! grep -qaF "$stamp" "$ARTIFACTS_DIR/lib/libwebrtc.a"; then
    echo "FATAL: stamp $stamp missing from $ARTIFACTS_DIR/lib/libwebrtc.a (patch not applied?)" >&2
    exit 1
  fi
  echo "stamp OK: $stamp"
done

cp "$OUTPUT_DIR/obj/webrtc.ninja" "$ARTIFACTS_DIR"
cp "$OUTPUT_DIR/obj/modules/desktop_capture/desktop_capture.ninja" "$ARTIFACTS_DIR"
cp "$OUTPUT_DIR/args.gn" "$ARTIFACTS_DIR"

rm -rf "$ARTIFACTS_DIR/include"
cd src
find . -name "*.h" -print | cpio -pd "$ARTIFACTS_DIR/include"
find . -name "*.inc" -print | cpio -pd "$ARTIFACTS_DIR/include"
cd ..
echo "=== DONE: $ARTIFACTS_DIR ==="
