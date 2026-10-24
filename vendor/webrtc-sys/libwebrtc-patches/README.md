# Patched libwebrtc — AV1 in encrypted calls + frame-parallel dav1d

Two patches ride on the rebuilt libwebrtc. Both must be applied before
running the build runners; both carry grep-able vintage stamps so a stale
archive can never masquerade as current (`strings <binary>` must show
`tinhat-av1-e2ee-v2` AND `tinhat-dav1d-fp-v1`).

Our `webrtc-sys` normally links the LiveKit prebuilt `libwebrtc` (release tag
`webrtc-51ef663` — named after the LAST 7 hex chars of webrtc-sdk/webrtc commit
`aaeeee8077eb0a4cad1c9494e9c6433c751ef663`, branch `m144_release` as of
2026-05-12). That prebuilt's `FrameCryptorTransformer` encrypts the entire AV1
payload with no OBU handling, so the AV1 RTP packetizer/depacketizer rewrites
"headers" it finds inside the ciphertext and AES-GCM refuses every frame:
`TINHAT_E2EE_CODEC=av1` → 0 frames, `DecryptionFailed`. Upstream never
implemented AV1 E2EE (livekit-client JS throws "av1 is not yet supported for
end to end encryption").

## The fix — `av1-e2ee-wrapper-obu.patch`

A byte-count carve-out cannot work: the depacketizer rewrites every OBU header
(`obu_has_size_field=1` + fresh LEB128), so even unencrypted AV1 bytes do not
round-trip the RTP hop byte-identically. Instead:

- **encryptFrame**: encrypt the ENTIRE original frame, then prepend one
  synthetic `OBU_FRAME` header byte (`0x30`, no extension, no size field). The
  packetizer sees a single OBU whose payload (ciphertext+IV+trailer) is opaque
  and runs to end-of-payload.
- **decryptFrame**: the wire form arrives as `0x32 + LEB128 + ciphertext…`
  (depacketizer normalization); parse and DISCARD the wrapper, decrypt the
  rest. The decoder gets bytes identical to the encoder output.
- The wrapper is not part of the ciphertext or the AAD (`frame_header` stays
  empty for AV1 on both sides), so both directions agree with any build of
  this same patch. H264/VP8/VP9/H265 paths are untouched.

Tin Hat Web's patched livekit-client E2EE worker implements the same wrapper
(Chrome's packetizer behaves identically), so web and desktop interoperate.
The stamps below are build markers only: they never travel on the wire, so
renaming them does not change the wire format.

## The second fix — `dav1d-frame-parallel.patch`

Stock `dav1d_decoder.cc` pins `max_frame_delay = 1` "for low latency", which
serialises decode of single-tile AV1 (NVENC's default bitstream) onto one
core no matter how many `n_threads` are configured — a 3936x1648 stream tops
out ~19 fps on a current laptop while the other cores idle. The patch enables
frame-parallel decode (default `max_frame_delay=3`, `n_threads` = max(SDK
cores, hardware_concurrency)), which requires three corrections to the stock
decode path, each load-bearing:

- **EAGAIN is flow control, not an error.** `dav1d_send_data` EAGAIN → drain
  pictures and retry the same data; `dav1d_get_picture` EAGAIN → no frame
  finished yet, return OK without output. Stock treats any nonzero return as
  `CODEC_ERROR`; with a deeper pipeline that becomes a PLI loop delivering
  ~1 keyframe/second (the 0.7.2 field regression).
- **dav1d must own the compressed bytes.** Stock wraps the caller's
  `EncodedImage` buffer, which dav1d keeps reading after `Decode()` returns
  once frames stay in flight — a use-after-free. The patch copies into a
  `dav1d_data_create` allocation.
- **Timestamps ride through dav1d.** Stock stamps the output picture with the
  *input* frame's RTP timestamp (1-in-1-out assumption); pipelined, that
  offsets every frame's identity by the pipeline depth. The patch carries the
  RTP timestamp in `Dav1dData.m.timestamp` and recovers ntp time from an
  input-order queue.

Env knobs: `TINHAT_DAV1D_FRAME_DELAY` (1..8; **1 = stock synchronous decode,
the kill-switch**), `TINHAT_DAV1D_THREADS`. `[dav1d-rt] fed/out/depth` counters
print every ≥5 s, mirroring `[nvenc-av1-rt]` on the encode side.

## Rebuilding (how this was produced)

On the build host: clone `livekit/rust-sdks` at tag `webrtc-51ef663`, cd
`webrtc-sys/libwebrtc`, pin `.gclient` to
`webrtc-sdk/webrtc.git@aaeeee8077eb0a4cad1c9494e9c6433c751ef663`, clone
depot_tools, `gclient sync -D --no-history`. Apply the recipe's own patches
(the lists inside `build_linux.sh` / `build_windows.cmd`, including the
`git -C build` / `third_party` / `third_party/libyuv` ones on Linux), apply
`av1-e2ee-wrapper-obu.patch` and `dav1d-frame-parallel.patch` (`git apply`
from `src/`; if a future upstream stops taking them, port the hunks by hand
and keep the stamps), then copy `build-patched.sh` (Linux) /
`win-build-patched.cmd` (Windows) into the recipe's `webrtc-sys/libwebrtc`
directory and run it there. They are the recipe's build scripts minus patch
application (idempotent) plus fail-fast compiles of the patched cryptor and
dav1d objects, and they refuse an archive that lacks either stamp. The
artifact directory defaults to `linux-x64-release` / `win-x64-release`;
set `WEBRTC_ARTIFACTS_DIR` (a folder of its own, outside `src/`) to write a
new archive beside an existing one instead of over it. `packaging/PKGBUILD`
links `webrtc-sys/libwebrtc/linux-x64-release-tinhat` under the recipe
checkout unless `LK_CUSTOM_WEBRTC` is already set, so build that one with
`WEBRTC_ARTIFACTS_DIR=<recipe>/webrtc-sys/libwebrtc/linux-x64-release-tinhat`.

Then build the app with the env var pointing at the artifact dir:

    LK_CUSTOM_WEBRTC=$HOME/rust-sdks-webrtc-recipe/webrtc-sys/libwebrtc/linux-x64-release cargo build --release

## Maintenance debt — read this before bumping webrtc-sys

Any `webrtc-sys`/`livekit` bump re-downloads the upstream prebuilt and
SILENTLY DROPS this patch unless the new tag's libwebrtc is rebuilt with it
and `LK_CUSTOM_WEBRTC` re-pointed. The guard is the E2EE loopback gate
(`TINHAT_E2EE_CODEC=av1` must PASS, its no-key negative control must stay
`MissingKey`) — run it after every dependency bump before shipping.
