#!/bin/bash
# run-tinhat-tx-test.sh ARTIFACT_DIR [-v] — compile tinhat_tx_test.cc against a BUILT libwebrtc archive (the folder
# build-patched.sh / win-build-patched.cmd wrote: lib/libwebrtc.a, include/, webrtc.ninja) and run it. It drives that
# archive's own FrameCryptorTransformer (the sender's length handling, stamp tinhat-tx-len-v1), so it tests the sender
# that ships, on the shared vectors testvectors/tx-len/vectors.json as well. Needs a C++20 compiler + OpenSSL headers.
# Exit 0 PASS, 1 sender FAIL, 2 harness/control problem.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
A=$(cd "${1:?usage: run-tinhat-tx-test.sh ARTIFACT_DIR [-v]}" && pwd); shift
test -f "$A/lib/libwebrtc.a" && test -d "$A/include" && test -f "$A/webrtc.ninja" || { echo "not a libwebrtc artifact dir: $A"; exit 2; }
# the archive's own feature defines (H.265 on or off, POSIX/Linux), nothing that changes the ABI
defs=$(sed -n 's/^defines = //p' "$A/webrtc.ninja" | head -1 | grep -oE -- '-D(WEBRTC|RTC|ABSL)_[A-Za-z0-9_]+(=[^ ]*)?|-DNDEBUG' | tr '\n' ' ')
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
# shellcheck disable=SC2086
${CXX:-g++} -std=c++20 -O1 -w $defs -I"$A/include" -I"$A/include/third_party/abseil-cpp" \
  "$here/tinhat_tx_test.cc" "$A/lib/libwebrtc.a" -lcrypto -lpthread -ldl -o "$t/tinhat_tx_test"
"$t/tinhat_tx_test" --vectors "$here/../../../../testvectors/tx-len/vectors.json" "$@"
