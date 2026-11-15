#!/usr/bin/env bash
# mpvqt-static.sh --fetch | [PREFIX] — build mpvqt (the watch-party player item, MpvAbstractItem) as a STATIC library
# for the Linux release .bin, and print the line that makes crates/app/build.rs link it:
#   export TINHAT_MPVQT_STATIC=PREFIX
#
#   --fetch   download the two pinned source tarballs (+ their .sig) into the inputs folder, once, and check them.
#   [PREFIX]  build from the inputs folder with NO network: each tarball must match its pinned sha256, or nothing is
#             built. Inputs folder: TINHAT_MPVQT_INPUTS (default ~/.cache/tinhat-mpvqt-static/inputs); keep it with
#             the release's other build inputs.
#
# Why (12.1.16): the .bin used to link the system's libMpvQt.so. Arch's mpvqt 1.2.1 changed the library's SONAME
# (libMpvQt.so.3 -> .so.4), so on a fully updated Arch the 12.1.11 - 12.1.13 .bins don't start at all. Linking mpvqt
# into the binary removes that dependency; libmpv.so.2 stays a normal shared dependency (its SONAME is stable).
# A folder of bundled .so files ($ORIGIN) doesn't fit a single-file download, and a run-time loader fallback can't
# cover a C++ class (MpvAbstractItem) that the app registers as a QML type.
#
# The pins: mpvqt 1.2.0 = the code every published .bin has run (1.2.1 adds signals + null checks and the new
# SONAME; moving to it is a separate change). ECM (KDE's CMake modules, needed only to configure mpvqt) is built into
# the same private prefix, so no package on the build machine changes. Both are KDE's own release tarballs (signed by
# their maintainers; the same files as the git tags v1.2.0 / v6.30.0), pinned by sha256.
#
# Licence: mpvqt is LGPL-2.1-only OR LGPL-3.0-only. Static linking is allowed when the user can relink the program
# with a changed mpvqt: Tin Hat's full source and build steps are public (BUILDING.md, this script), and this script
# copies the exact library source (KDE's mpvqt-1.2.0.tar.xz + .sig) and the two licence texts into PREFIX for the
# release to publish next to the .bin. See THIRD-PARTY-NOTICES.md ("mpvqt in the Linux download").
#
# Optional: JOBS (default 8).
# The default PREFIX is named after the pins AND this machine's compiler, Qt and architecture, so a prefix built
# against another Qt is never reused. Re-running with a finished PREFIX at the same identity only checks it (every
# file, the archive's recorded hash) and prints the export line.
set -euo pipefail
ARG=${1:-}
MPVQT_VER=1.2.0
MPVQT_TXZ=mpvqt-1.2.0.tar.xz
MPVQT_SHA256=8660ad79c0d60fed77f29b36e1742841466af5405de702c81a121e6eeb625ebb
MPVQT_URL=https://download.kde.org/stable/mpvqt/mpvqt-1.2.0.tar.xz
MPVQT_SIG_SHA256=4c1d5c3214387622d54ecdcd8ee84d56c62198646301b35224f1526c1d57e678   # KDE's signature, published with it
ECM_VER=6.30.0
ECM_TXZ=extra-cmake-modules-6.30.0.tar.xz
ECM_SHA256=22c9f7ff930dae7329faebf2942d2424f4a298c43bb3f11e217be6b12f227f6e
ECM_URL=https://download.kde.org/stable/frameworks/6.30/extra-cmake-modules-6.30.0.tar.xz
ECM_SIG_SHA256=16d5993d3acb6c2fe4e85da0ca7cbd7bbab3c6ae6bf4f48b297363f2ffef0392
IN=${TINHAT_MPVQT_INPUTS:-${XDG_CACHE_HOME:-$HOME/.cache}/tinhat-mpvqt-static/inputs}
JOBS=${JOBS:-8}
case "$JOBS" in ''|*[!0-9]*) echo "mpvqt-static: JOBS must be a number" >&2; exit 2;; esac

# sha_ok FILE SHA256 -> 0 when FILE exists and has exactly that sha256
sha_ok() { [ -f "$1" ] && [ "$(sha256sum "$1" | cut -c1-64)" = "$2" ]; }

if [ "$ARG" = --fetch ]; then
  command -v curl >/dev/null || { echo "mpvqt-static: curl is missing" >&2; exit 2; }
  mkdir -p "$IN"
  # each file on its own: the tarballs and their signatures, each kept only when it matches its pin
  for spec in "$MPVQT_TXZ $MPVQT_SHA256 $MPVQT_URL" "$MPVQT_TXZ.sig $MPVQT_SIG_SHA256 $MPVQT_URL.sig" \
              "$ECM_TXZ $ECM_SHA256 $ECM_URL" "$ECM_TXZ.sig $ECM_SIG_SHA256 $ECM_URL.sig"; do
    set -- $spec
    if sha_ok "$IN/$1" "$2"; then echo "mpvqt-static: $IN/$1 is there and matches" >&2; continue; fi
    curl -fsSL -m 300 -o "$IN/$1.part" "$3" || { rm -f "$IN/$1.part"; echo "mpvqt-static: download of $3 failed" >&2; exit 4; }
    sha_ok "$IN/$1.part" "$2" || { echo "mpvqt-static: $3 does not match the pinned sha256 $2: not kept" >&2; rm -f "$IN/$1.part"; exit 3; }
    mv -f "$IN/$1.part" "$IN/$1"; echo "mpvqt-static: fetched $IN/$1 (sha256 matches)" >&2
  done
  exit 0
fi
for t in tar xz cmake nm sha256sum pkg-config flock realpath; do command -v "$t" >/dev/null || { echo "mpvqt-static: $t is missing" >&2; exit 2; }; done
# the build never touches the network: the pinned inputs must be here, byte for byte
for spec in "$MPVQT_TXZ $MPVQT_SHA256" "$MPVQT_TXZ.sig $MPVQT_SIG_SHA256" "$ECM_TXZ $ECM_SHA256"; do
  set -- $spec
  [ -f "$IN/$1" ] || { echo "mpvqt-static: no $IN/$1: run scripts/mpvqt-static.sh --fetch once (or copy the release's build inputs there)" >&2; exit 3; }
  sha_ok "$IN/$1" "$2" || { echo "mpvqt-static: $IN/$1 does not match the pinned sha256 $2: STOP" >&2; exit 3; }
done
set --

# What the build really uses, named explicitly (CMake gets exactly these; CXX and CMAKE_PREFIX_PATH from the caller's
# environment are not passed on) and recorded in the prefix's identity: the compiler, its target, Qt's version and
# the Qt CMake package it is found through.
CXXC=$(command -v "${CXX:-c++}") || { echo "mpvqt-static: no C++ compiler (${CXX:-c++})" >&2; exit 2; }
CXX_REAL=$(realpath "$CXXC")   # identity only: a wrapper (ccache's g++ link) must still be called by its own path
CXX_ID=$("$CXXC" --version | head -n1)
TARGET=$("$CXXC" -dumpmachine)
QT_VER=$(pkg-config --modversion Qt6Core) || { echo "mpvqt-static: pkg-config Qt6Core failed" >&2; exit 2; }
QT6_DIR=$(realpath "$(pkg-config --variable=libdir Qt6Core)/cmake/Qt6") || { echo "mpvqt-static: no Qt6 CMake package" >&2; exit 2; }
[ -f "$QT6_DIR/Qt6Config.cmake" ] || { echo "mpvqt-static: no $QT6_DIR/Qt6Config.cmake" >&2; exit 2; }
IDENT="mpvqt $MPVQT_VER $MPVQT_SHA256 static pic; ecm $ECM_VER $ECM_SHA256; qt $QT_VER at $QT6_DIR; $TARGET; $CXXC -> $CXX_REAL $CXX_ID"
IDHASH=$(printf '%s' "$IDENT" | sha256sum | cut -c1-12)
MARK=.tinhat-mpvqt-static   # in every prefix this script made, from the first moment (a failed build can be retried)

PREFIX=${ARG:-${XDG_CACHE_HOME:-$HOME/.cache}/tinhat-mpvqt-static/v$MPVQT_VER-qt$QT_VER-$IDHASH}
case "$PREFIX" in /*) ;; *) echo "mpvqt-static: PREFIX must be an absolute path" >&2; exit 2;; esac
# plain path characters only: the prefix ends up in compiler flags and in a shell export line
case "$PREFIX" in *[!A-Za-z0-9._/+-]*) echo "mpvqt-static: PREFIX may hold only letters, digits and . _ / + -" >&2; exit 2;; esac
# one spelling per folder (no //, ./, .., trailing /, symlinked parents), so the lock below is the same for everyone
PREFIX=$(realpath -m -- "$PREFIX")
case "$PREFIX" in /|*[!A-Za-z0-9._/+-]*) echo "mpvqt-static: PREFIX resolves to '$PREFIX': not usable" >&2; exit 2;; esac
mkdir -p "$(dirname "$PREFIX")"

# one build per prefix at a time
exec 9>"$PREFIX.lock"
flock -w 1800 9 || { echo "mpvqt-static: $PREFIX.lock is held by another build" >&2; exit 2; }

A="$PREFIX/lib/libMpvQt.a"
SRC_TGZ="$PREFIX/$MPVQT_TXZ"
# what the app's build reads + what the release publishes; PIN records a sha256 for each, and every one must match
NEED=(lib/libMpvQt.a include/MpvQt/MpvAbstractItem include/MpvQt/MpvController include/MpvQt/mpvabstractitem.h
      include/MpvQt/mpvcontroller.h include/MpvQt/mpvqt_export.h include/MpvQt/mpvqt_version.h
      "$MPVQT_TXZ" "$MPVQT_TXZ.sig" LGPL-2.1-only.txt LGPL-3.0-only.txt)
finished() {
  [ -f "$PREFIX/$MARK" ] && [ -f "$PREFIX/PIN" ] || return 1
  [ "$(head -n1 "$PREFIX/PIN")" = "$IDENT" ] || return 1
  local f n=0
  for f in "${NEED[@]}"; do [ -s "$PREFIX/$f" ] || return 1; done
  # every file PIN lists still has its hash, and PIN lists every file the build needs
  (cd "$PREFIX" && sed -n 's/^file //p' PIN | sha256sum -c --status) || return 1
  for f in "${NEED[@]}"; do grep -q "^file [0-9a-f]\{64\}  $f\$" "$PREFIX/PIN" && n=$((n + 1)); done
  [ "$n" = "${#NEED[@]}" ]
}
if finished; then
  echo "mpvqt-static: $PREFIX is already built and checks out (mpvqt $MPVQT_VER, Qt $QT_VER)" >&2
  printf 'export TINHAT_MPVQT_STATIC=%q\n' "$PREFIX"
  exit 0
fi
# only a folder this script made (its marker) is ever emptied; anything else non-empty is refused and left alone
if [ -e "$PREFIX" ] && [ -n "$(ls -A "$PREFIX" 2>/dev/null)" ] && [ ! -f "$PREFIX/$MARK" ]; then
  echo "mpvqt-static: $PREFIX exists, isn't empty and isn't one of this script's prefixes: refusing to build into it" >&2
  exit 2
fi

W=$(mktemp -d "$PREFIX.work.XXXXXX")
trap 'rm -rf -- "$W"' EXIT
rm -rf -- "$PREFIX"
mkdir -p "$PREFIX"
echo "made by scripts/mpvqt-static.sh; safe to delete" >"$PREFIX/$MARK"

# the pinned tarballs (checked above), unpacked into the work dir
tar -xJf "$IN/$ECM_TXZ" -C "$W" && tar -xJf "$IN/$MPVQT_TXZ" -C "$W" || { echo "mpvqt-static: unpacking the inputs failed" >&2; exit 3; }
ECM_DIR="$W/extra-cmake-modules-$ECM_VER"
MPVQT_DIR="$W/mpvqt-$MPVQT_VER"
[ -f "$ECM_DIR/CMakeLists.txt" ] && [ -f "$MPVQT_DIR/CMakeLists.txt" ] || { echo "mpvqt-static: the inputs don't unpack to the expected folders" >&2; exit 3; }

# ECM: CMake files only, into the private prefix (no docs, no tests).
CM=(env -u CXX -u CC -u CMAKE_PREFIX_PATH -u Qt6_DIR cmake)
"${CM[@]}" -S "$ECM_DIR" -B "$W/ecm-build" -DCMAKE_INSTALL_PREFIX="$PREFIX/ecm" -DBUILD_TESTING=OFF \
  -DBUILD_HTML_DOCS=OFF -DBUILD_MAN_DOCS=OFF -DBUILD_QTHELP_DOCS=OFF >"$W/ecm-configure.log" 2>&1 \
  || { tail -n 30 "$W/ecm-configure.log" >&2; exit 4; }
"${CM[@]}" --build "$W/ecm-build" --target install >"$W/ecm-install.log" 2>&1 || { tail -n 30 "$W/ecm-install.log" >&2; exit 4; }

# mpvqt: static, position-independent (the app links as a PIE), release-optimised, no debug info; the build paths
# are mapped away like the app's own (scripts/release-paths.sh).
MAP="-ffile-prefix-map=$MPVQT_DIR=/mpvqt -ffile-prefix-map=$W/mpvqt-build=/mpvqt-build"
"${CM[@]}" -S "$MPVQT_DIR" -B "$W/mpvqt-build" -DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=OFF \
  -DCMAKE_CXX_COMPILER="$CXXC" -DQt6_DIR="$QT6_DIR" \
  -DCMAKE_POSITION_INDEPENDENT_CODE=ON -DCMAKE_PREFIX_PATH="$PREFIX/ecm" -DCMAKE_INSTALL_PREFIX="$PREFIX" \
  -DKDE_INSTALL_LIBDIR=lib -DBUILD_TESTING=OFF -DBUILD_EXAMPLES=OFF -DCMAKE_CXX_FLAGS="$MAP" \
  >"$W/mpvqt-configure.log" 2>&1 || { tail -n 30 "$W/mpvqt-configure.log" >&2; exit 5; }
"${CM[@]}" --build "$W/mpvqt-build" -j "$JOBS" >"$W/mpvqt-build.log" 2>&1 || { tail -n 30 "$W/mpvqt-build.log" >&2; exit 5; }
"${CM[@]}" --install "$W/mpvqt-build" >"$W/mpvqt-install.log" 2>&1 || { tail -n 30 "$W/mpvqt-install.log" >&2; exit 5; }

[ -f "$A" ] || { echo "mpvqt-static: no $A after the install" >&2; exit 6; }
found=$(sed -n 's/^Qt6Quick_DIR:PATH=//p' "$W/mpvqt-build/CMakeCache.txt")
[ -n "$found" ] && [ "$(realpath -m -- "$found")" = "$(dirname "$QT6_DIR")/Qt6Quick" ] \
  || { echo "mpvqt-static: CMake found Qt6Quick at '$found', not next to $QT6_DIR" >&2; exit 6; }
nm -C "$A" >"$W/nm.txt" 2>/dev/null || { echo "mpvqt-static: nm failed on $A" >&2; exit 6; }
grep -qF "T MpvAbstractItem::MpvAbstractItem" "$W/nm.txt" || { echo "mpvqt-static: $A has no MpvAbstractItem" >&2; exit 6; }
if grep -qaF -- "$W" "$A"; then echo "mpvqt-static: $A still names the build folder" >&2; exit 6; fi

# The exact library source + its licence texts, for the release's LGPL source offer.
cp "$IN/$MPVQT_TXZ" "$SRC_TGZ"
cp "$IN/$MPVQT_TXZ.sig" "$PREFIX/"
cp "$MPVQT_DIR/LICENSES/LGPL-2.1-only.txt" "$MPVQT_DIR/LICENSES/LGPL-3.0-only.txt" "$PREFIX/"

# The stamp last, written whole: a prefix without it is never reused.
{ echo "$IDENT"
  echo "built $(date -u +%Y-%m-%dT%H:%M:%SZ) with $(cmake --version | head -n1)"
  echo "libMpvQt.a sha256 $(sha256sum "$A" | cut -c1-64)"
  echo "source tarball sha256 $(sha256sum "$SRC_TGZ" | cut -c1-64)"
  (cd "$PREFIX" && sha256sum "${NEED[@]}" | sed 's/^/file /')
} >"$W/PIN"
mv -f "$W/PIN" "$PREFIX/PIN"
finished || { echo "mpvqt-static: the finished prefix doesn't check out" >&2; exit 6; }
echo "mpvqt-static: built $A (mpvqt $MPVQT_VER, Qt $QT_VER)" >&2
printf 'export TINHAT_MPVQT_STATIC=%q\n' "$PREFIX"
