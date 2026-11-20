# Patched libwebrtc — AV1 in encrypted calls + frame-parallel dav1d + VP9 keyframes the SFU can see

Three patches ride on the rebuilt libwebrtc. All must be applied before
running the build runners; each carries a grep-able vintage stamp so a stale
archive can never masquerade as current (`strings <binary>` must show
`tinhat-av1-e2ee-v2`, `tinhat-dav1d-fp-v1`, `tinhat-vp9-kf-iv-v1`,
`tinhat-iv-v2`, `tinhat-iv-rx-v1` AND `tinhat-tx-len-v1`; the third patch carries the last four).

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

## The third fix — `vp9-e2ee-keyframe-iv.patch` (12.1.8, share-never-decodes)

VP9 frames are encrypted from byte 0 (`get_unencrypted_bytes` → 0, the same rule
as every LiveKit client, so it can't change without breaking every older
receiver). The LiveKit SFU, though, decides "keyframe" from byte 0 of the VP9
payload (`mediatransportutil` `codec.IsVP9KeyFrame`: frame marker, profile,
show_existing_frame, frame_type). On ciphertext a frame is taken
for a keyframe with probability 1/16 whatever it is: real keyframes are missed,
delta frames pass. The SFU's
forwarder only starts a subscriber at a keyframe, so a share either started on a
delta frame (the viewer waits for the next real keyframe: slow first frame, a
PLI storm) or never started (no "keyframe" in the whole share: the viewer gets
nothing, the SFU logs a PLI "for layer lock" every 200 ms and its buffer's
`keyFrames` stays 0 while the sender made 11). At software-VP9 frame rates on a
static screen (4–8 fps, ~48 frames in 6 s) the chance that no frame of a share
is taken for a keyframe is ≈ (15/16)^48 ≈ 4.5 %, the measured never-decoded
rate (an estimate of the never-started case, not a model of every failure).

The fix is on the SENDER, wire-compatible, in two parts (both in `vp9-e2ee-keyframe-iv.patch`; the logic lives
in the new header `api/crypto/tinhat_iv.h`):

- **The IV allocator (`tinhat-iv-v2`; v1 was the 12.1.8 candidate that broke H.264, below).** Every sender IV is
  `domain (4 bytes) ‖ counter (8 bytes)`, **every byte nonzero, byte 0 ≥ 4** (H.264-safe, below), from ONE
  allocator per process (`tinhat::ProcessIvAllocator()`: every cryptor, SSRC, layer and room of the process).
  Each counter value is handed out once; it never wraps (at 255^8 it refuses and the frame is dropped). This
  replaces upstream's `ssrc ‖ ts ‖ ts − count % 0xFFFF` per-cryptor counter, which could repeat after a same-key
  reconnect (and whose native random start was never used). Since a Tin Hat call key is minted per join or
  rotation inside one process and never persisted, no (key, IV) pair can repeat. The full argument (every case:
  retries, equal timestamps, SVC/simulcast, rotation, ratchet, reconnect, restart) is in share
  `claude/tinhat-desktop/never-decodes/README.md`, section (1).
- **The VP9 keyframe IV (`tinhat-vp9-kf-iv-v1`).** AES-GCM is counter mode (ciphertext byte 0 = plaintext byte 0
  XOR keystream byte 0) and the IV rides in the frame trailer. So `encryptFrame` takes candidates from the
  allocator (`tinhat::StepVp9Iv`) until byte 0 of the ciphertext tells the SFU the truth: keyframe ⇔ looks like
  one.
  - Each try seals ONE byte privately (with a 96-bit IV it equals byte 0 of the real ciphertext; its tag is
    discarded). That is ~16 tries per keyframe and ~1.07 per delta frame, capped at 512.
  - **Only a matching IV is ever sent:** a cap miss (p = (15/16)^512 for a keyframe), a probe that fails to seal, or
    an exhausted allocator drops the frame. The stream recovers when a later keyframe is delivered, normally the next PLI's (lab: 24/24
    forced drops were followed by an SFU PLI and a forwarded keyframe).
  - Byte 0 then tells the SFU the keyframe bit and nothing more about plaintext byte 0 (computationally, given that
    bit); the clear VP9 RTP descriptor (P bit) and frame sizes largely show that bit already.
  - Cost: ~8 µs per keyframe whatever its size (1080p to 4K measured), next to 35–92 µs for the keyframe's real seal.
- Receivers are untouched; any receiver that decrypts VP9 today decrypts a new sender's frames as before (only
  the IV value differs; lab interop in both directions, desktop and web).
- **Tests:**
  - `tests/run-tinhat-iv-test.sh`: compiles `tinhat_iv.h` AS SHIPPED (taken from the patch) with
    `tests/tinhat_iv_test.cc` against OpenSSL AES-GCM and checks:
    - 300,000 sent IVs of a long session with the counter forced across its 32-bit carry: 0 repeats;
    - full-seal byte 0 = probe byte 0 = the SFU verdict;
    - the controls catch repeats; the exhaustion/cap/probe-failure drops; byte-0 uniformity.
  - Rust `vp9_kf_iv_tests` (vectors + the patch carries this rule, no lab knob).
- Tin Hat Web's patched E2EE worker implements the same allocator (one per worker = per call join) and the same
  rule, predicate and cap (`tinhatVp9KeyframeIv`); test `tools/iv-unique-unit.mjs` drives its real `encodeFunction`.
  Shared vectors: `testvectors/vp9-kf-iv/vectors.json` (byte-identical in both repos).
- Not affected: AV1 (the SFU reads the RTP aggregation header, outside the ciphertext), H.264/H.265 (NAL headers
  stay clear), VP8 (10/3 clear bytes). All of them get the new IV allocator.
- Records: share `claude/tinhat-desktop/never-decodes/README.md`.

## The H.264-safe IV layout (`tinhat-iv-v2`, 12.1.8)

The first 12.1.8 candidate (`tinhat-iv-v1`) wrote the counter as 64-bit big-endian from a start below 2^40, so IV
bytes 4–6 were always `00 00 00`. For H.264 (and H.265) the sender RBSP-escapes `ciphertext ‖ IV ‖ trailer`: a `0x03`
goes in before any byte ≤ 3 that follows two `0x00` (`H264::WriteRbsp`). Meanwhile every receiver in users' hands
(upstream `decryptFrame`) reads the IV from the RAW frame tail and only then unescapes the ciphertext. So every H.264
frame carried an escape inside its IV, every native receiver read a shifted IV, and AES-GCM refused the frame
silently. Upstream's IV almost never contains `00 00`, so this receiver behaviour was a latent upstream bug; the web
worker unescapes first and was never affected.

v2 writes **no zero byte**:
- the domain's random bytes are mapped into 1..255, with byte 0 in 4..255;
- the counter is written as 8 base-255 digits, each + 1, most significant first. That is a bijection on `c < 255^8`,
  so uniqueness is unchanged, and the cap is 255^8 ≈ 1.8·10^19 > 2^63.

Why no escape can then reach the 14 bytes the receiver reads raw (`IV ‖ ivLength ‖ keyIndex`):
- An escape before IV[k], k ≥ 2, needs IV[k−2] = IV[k−1] = 0: impossible.
- Before IV[1], it needs (the tag's last byte, IV[0]) = `00 00`: impossible.
- Before ivLength (12 > 3): never.
- Before the key index, it needs (IV[11], 12) = `00 00`: impossible.
- **The tag/IV junction:** the only escape the boundary could produce is "tag …`00 00` | IV[0] ≤ 3". IV[0] is ≥ 4, so it
  can't happen. Even if it did, the `0x03` would be inserted BEFORE IV[0], i.e. into the ciphertext side. The last 14
  bytes of the frame would still be exactly `IV ‖ trailer`, so the receiver's tail read would be unaffected.

H.265 uses the same RBSP rule and the same raw-tail read, so it is covered too. The receiver was hardened later
(next section); v2 senders don't need it, and it can't help receivers already in users' hands.

Tests (`tests/tinhat_iv_test.cc`, the shipped header against OpenSSL):
- no zero byte, byte 0 ≥ 4, no `00 00 [00–03]` run inside the IV, IV ‖ trailer or after any tag tail, over 300k IVs;
- the base-255 digit carries 255^0…255^8: zero-free, order-preserving, decode back; 200k IVs across 255^2 and 255^4
  are distinct;
- the H.264 escape → raw-tail read → unescape → decrypt round trip, 20000/20000 with v2 IVs, while the v1 layout fails
  20000/20000 (the control).
- Shared vectors: `testvectors/iv-layout/vectors.json` (Rust `iv_layout_matches_the_shared_vectors`, the web's
  `iv-unique-unit`).

## The receiver reads the IV after unescaping (`tinhat-iv-rx-v1`)

Upstream `decryptFrame` took the IV from the RAW frame tail and only then ran `ParseRbsp` over the rest. The sender
escapes `ciphertext ‖ IV ‖ trailer` as one run, so the raw read is wrong whenever the escape lands inside the IV: on
every frame of a v1 sender, and on the rare upstream IV that holds `00 00 [00–03]`. The receiver now:

- unescapes the whole body first (H.264/H.265; other codecs have no escaping), then takes ciphertext, IV and trailer
  from the unescaped bytes with `tinhat::SplitRxTail` (`api/crypto/tinhat_iv.h`). Those are exactly the bytes the
  sender sealed, whatever IV layout it used: upstream, v1 and v2 senders all decode.
- still reads the 2-byte trailer raw first (IV length, key index for the key lookup). The trailer is never escaped:
  an escape before T0 needs T0 ≤ 3 (it is 12); one before T1 needs (IV[last], T0) = `00 00`. So the raw trailer and
  the unescaped one are the same bytes (also checked).
- bounds checks: a frame too short for its clear header and trailer, or a body too short for tag ‖ IV ‖ trailer,
  is refused before any parsing (and again after unescaping); `NeedsRbspUnescaping` checks short bodies.
- The structural refusals report `DecryptionFailed` (a frame shorter than header + trailer now does so before the key
  lookup, where upstream reached `MissingKey`); they don't count towards `failure_tolerance` and never invalidate a
  key.

The Tin Hat Web worker (livekit-client's `decryptFrame`) already unescapes before it reads the IV: no web change.

Tests:
- `tests/run-tinhat-rx-test.sh <artifact dir>`: compiles `tests/tinhat_rx_test.cc` against a BUILT archive
  (`lib/libwebrtc.a` + `include/`) and drives that archive's own `FrameCryptorTransformer`. For H.264, H.265, VP8, VP9
  and AV1 it seals frames under the transformer's derived key with the upstream, v1, v2 and PLANTED IV layouts
  (`00 00 [00–03]` at every IV position), escapes them with the archive's `WriteRbsp`, sends the H.264 ones through
  the archive's RTP packetizer and depacketizer as well, and checks every frame decrypts byte-identically. It also
  round-trips each codec and audio through the archive's own sender (H.264 and AV1 also through the RTP hop: the AV1
  depacketizer's rewritten wrapper), covers tag/IV junction escapes and a nonzero key index, and feeds short and
  malformed frames per codec in a child process with a timeout. Negative sets (another key; a flipped tag,
  ciphertext, IV or clear-header byte; short frames) must deliver NOTHING. Controls: the v1 and planted sets must
  carry escapes inside the tail. ("upstream" = upstream's IV allocation inside Tin Hat's framing: its AV1 frames
  carry Tin Hat's wrapper.) **Shown able to fail:** on the previous archive (raw-tail receiver) H.264/H.265 v1 come
  back 0/300, planted 24/264, upstream 296/300.
  12.1.11 additions (review notes on the 12.1.10 hotfix): every TOTAL frame length 1..40 independent of the clear
  header, grown from truncated seeds (bare/truncated H.264 and H.265 NAL headers such as `00 00 00 01 65`, a one-byte
  VP8 keyframe, AV1 wrapper and LEB128 prefixes, audio TOC bytes; one child per seed), and the KEY STATE after
  refusals (failure_tolerance 0, and a ratchet window of 2 with the sender one step ahead: valid, 15 malformed, valid
  must leave the last frame decrypted, the key valid and unchanged, and the observer at exactly Ok/KeyRatcheted >
  DecryptionFailed > Ok; controls: a flipped tag under tolerance 0 does invalidate the key, under tolerance 1 it does
  not). **Shown able to fail:** on 1218d every non-AV1 seed dies (17/25) and the non-AV1 key-state cases fail (10/12:
  the next valid frame dropped, key invalid); AV1 passes there by nature (its old wrapper check already refused short
  frames without counting a failure). Crash-only: no instrumented (ASAN) archive is built.
- `tests/tinhat_iv_test.cc` section 10: `SplitRxTail` as shipped, unescape-first: v1 20000/20000, v2 20000/20000,
  planted 1100/1100, short bodies refused.
- Rust `the_libwebrtc_patch_carries_the_same_rule`: the receiver lines are in the patch and the raw-tail IV read is
  removed.

## The sender's length check (`tinhat-tx-len-v1`, 12.1.17)

Upstream `encryptFrame` copied the codec's clear header (`get_unencrypted_bytes`: the first H.264/H.265 slice NAL's
offset + 2, 10 bytes for a VP8 keyframe and 3 for a delta frame, 1 for audio, 0 for VP9 and AV1) without checking the
frame's length, then sized the sealed payload as `size − header`. A frame SHORTER than its own header (degenerate
output of our own encoder, e.g. H.264 `00 00 01 65` = header 5 of 4 bytes, or a VP8 keyframe under 10 bytes) read
past the end of the frame and then underflowed into a huge allocation, i.e. an abort of the whole app. Only the local
encoder's output reaches this path. The sender now:

- drops such a frame: never sent as is, never sealed. It logs the 1st, 2nd, 4th, 8th, … drop and leaves the cryptor
  state alone (no key problem: the next frame seals normally). The receivers have had the matching check
  (`size ≥ header + 2`) since `tinhat-iv-rx-v1`.
- seals a frame exactly as long as its header with an empty payload (tag ‖ IV ‖ trailer = 30 bytes), which every
  receiver accepts.
- keeps the header length a `uint8_t`, as upstream does on both sides, so the wire behaviour is unchanged. An H.264
  slice NAL at offset ≥ 254 wraps that length mod 256 on the sender and on every native receiver alike: such frames
  stay as they were (sealed; they decrypt only at offset 254, and the wrapped header is SHORTER than the real one, so
  no extra byte goes out in the clear). Moving both sides to `size_t` would be a wire change of its own.

Tin Hat Web's worker (livekit-client `encodeFunction`) had the same shape: its typed-array view of the header threw a
`RangeError` outside the per-frame `try` (no out-of-bounds read in JavaScript, nothing left unencrypted), which errored
the track's whole encode stream. It drops such a frame too since 12.1.17 (same stamp; tinhat-web
`src/features/calls/SENDER-LENGTH-CHECK.md`). Shared vectors: `testvectors/tx-len/vectors.json` (byte-identical in both
repos; they also record the web's header where its NAL scanner differs on tiny frames, pre-existing).

Tests:
- `tests/run-tinhat-tx-test.sh <artifact dir>` (Linux, g++) / `tests/run-tinhat-tx-test.ps1 <artifact dir>` (Windows,
  MSVC /MT, the same source): compiles `tests/tinhat_tx_test.cc` against a BUILT archive and drives that archive's own
  sender and receiver. Each set runs in its own process (POSIX fork; Windows: the test re-runs itself with `--job K`,
  crash dialogs off), and the independent AES-GCM check uses system OpenSSL on Linux (the archive's own BoringSSL is
  prefixed `LK_*` there, so it can't stand in for it) and CNG (bcrypt) on Windows. A truncated vectors file is refused. Every frame handed to the sender sits at the end of a guarded mapping (the
  next page is `PROT_NONE`), so a read of one byte past the frame kills the child (SIGSEGV) instead of passing
  unnoticed (control: a planted one-byte overread must fault). Per codec (H.264, H.265, VP8, VP9, AV1, audio), each
  set in its own child with a timeout:
  - SHORT: frames shorter than their clear header must deliver NOTHING, the sender must survive, and a valid frame
    after them must still be sealed;
  - BOUNDARY: header, header + 1, header + 2 bytes (1–3 for VP9/AV1), key and delta frames, under key index 0 and 3:
    sealed, the clear header kept as is (AV1: the wrapper), the trailer `[12, key index]`, the exact sealed length
    (H.264/H.265: at least that), no plaintext run in the output, decrypted byte-identical by the archive's receiver;
  - OVERSIZED: 1, 4 and 16 MiB frames, the same checks;
  - H.264 WIDE HEADER: slice NAL offsets 251–253 (header 253–255) must round-trip; offsets 254–600 must be survived,
    end in the trailer, and open INDEPENDENTLY (system OpenSSL AES-GCM under the derived key, AAD = the wrapped clear
    header, body unescaped by the archive's `ParseRbsp`) to exactly the rest of the frame: no byte past the clear
    header goes out unsealed (whether the archive's receiver decrypts them is printed, not asserted);
  - VECTORS: every case of the shared vectors, each in its own child (drop: nothing delivered; seal: the BOUNDARY
    checks + byte-identical decryption), always followed by a valid frame that must seal AND decrypt (the SHORT sets'
    follow-up frame is decrypted as well).
  **Shown able to fail:** on the 12.1.16 archive (`linux-x64-release-tinhat-12110-ivrx3`) the H.264, H.265 and VP8
  SHORT children and all 22 "drop" vectors die (SIGSEGV: the read past the frame); on an archive whose check is off
  by one (`<=`, a plant) the BOUNDARY sets and the 7 "exactly the header" vectors fail (dropped).
- Rust `the_sender_drops_frames_shorter_than_their_header`: the check is in the patch, ahead of the header copy and
  the payload sizing; `tx_len_vectors_match_the_patch`: the shared vectors carry the patch's stamp and every case's
  verdict follows from its length and header.

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
dav1d objects, and they refuse an archive that lacks any of the stamps. The
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
