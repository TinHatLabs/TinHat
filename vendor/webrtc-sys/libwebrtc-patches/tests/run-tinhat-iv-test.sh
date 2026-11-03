#!/bin/bash
# run-tinhat-iv-test.sh — compile api/crypto/tinhat_iv.h AS SHIPPED (taken from ../vp9-e2ee-keyframe-iv.patch) with
# tinhat_iv_test.cc against OpenSSL and run it. Needs a C++17 compiler + OpenSSL headers (the build machine has both).
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
( cd "$t" && git init -q . && git apply --include=api/crypto/tinhat_iv.h "$here/../vp9-e2ee-keyframe-iv.patch" )
test -f "$t/api/crypto/tinhat_iv.h" || { echo "FAIL: no tinhat_iv.h in the patch"; exit 1; }
${CXX:-g++} -std=c++17 -O2 -Wall -Wextra -I"$t" "$here/tinhat_iv_test.cc" -o "$t/tinhat_iv_test" -lcrypto
"$t/tinhat_iv_test"
