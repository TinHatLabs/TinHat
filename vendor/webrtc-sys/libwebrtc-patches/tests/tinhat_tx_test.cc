// tinhat_tx_test.cc — the E2EE SENDER's length handling (stamp tinhat-tx-len-v1, 12.1.17), through the REAL libwebrtc
// archive: its own FrameCryptorTransformer::encryptFrame and get_unencrypted_bytes, and its own decryptFrame for the
// round trips.
//
// Every frame handed to the sender sits at the very END of a guarded mapping (the next page is PROT_NONE / PAGE_NOACCESS),
// so a read of even one byte past the frame kills the process (SIGSEGV / access violation) instead of passing
// unnoticed. Each set runs in its own child with a timeout (POSIX: fork; Windows: the test re-runs itself with
// --job K); a signal / fault / abort = the sender died. Builds with g++ (Linux, run-tinhat-tx-test.sh) and MSVC
// (Windows, run-tinhat-tx-test.ps1); the independent AES-GCM check uses system OpenSSL there and CNG (bcrypt) here.
// Sets per codec (H.264, H.265 when the archive has it, VP8, VP9, AV1, audio):
//   - SHORT: frames shorter than their own clear header (H.264/H.265 `00 00 01 <slice NAL header>` and the 4-byte
//     start code forms, parameter sets + a bare slice NAL header, VP8 keyframes of 1..9 bytes and delta frames of
//     1..2). Nothing may be delivered (never sent as is, never sealed), the sender must survive, and a valid frame
//     after them must still be sealed. Upstream's sender (the 12.1.16 archive) dies here: it reads past the frame and
//     then sizes the payload as size - header (an unsigned underflow). VP9, AV1 (no clear header) and audio (1 byte)
//     can't be shorter than their header: empty frames never reach the sealing code.
//   - BOUNDARY: header, header + 1 and header + 2 bytes (1, 2, 3 bytes for VP9/AV1), key and delta frames, under key
//     index 0 and 3. Each must be sealed, keep exactly its clear header in front (AV1: the wrapper), end in the
//     trailer [12, key index], have the exact sealed length (H.264/H.265: at least that, the body is RBSP-escaped)
//     and decrypt byte-identical through the archive's receiver. A check that is off by one (drops a frame exactly as
//     long as its header) fails here.
//   - OVERSIZED: 1, 4 and 16 MiB frames are sealed with the right header, trailer and length, carry no run of their
//     plaintext body, and decrypt byte-identical.
//   - H.264 WIDE HEADER: the slice NAL at offsets 251..253 (clear header 253..255, the most the uint8_t header
//     length holds) must round-trip; at offsets 254..600 the header length wraps mod 256 (upstream's rule on BOTH
//     sides, the wire behaviour is unchanged): the sender must survive, and every delivered frame is opened
//     INDEPENDENTLY (system OpenSSL AES-GCM under the transformer's derived key, AAD = the wrapped clear header, body
//     unescaped with the archive's ParseRbsp): it must authenticate and give back exactly the rest of the frame, so no
//     byte past the clear header went out unsealed. Whether the archive's own receiver decrypts them is printed (INFO).
//   - VECTORS: every case of the shared vectors testvectors/tx-len/vectors.json (byte-identical in tinhat-web, whose
//     tools/tx-len-unit.mjs runs them through the web sender), each in its own child: "drop" = nothing delivered and
//     the next valid frame sealed + decrypted; "seal" = the checks of BOUNDARY + byte-identical decryption.
//   The SHORT sets' follow-up frame is decrypted too (a sender that drops a short frame but breaks the next one fails).
// Controls: reading one byte past a guarded frame must kill a child (the guard bites); every run's sentinel must come
// back.
//
// Build + run: run-tinhat-tx-test.sh <libwebrtc artifact dir> [-v] (the directory with lib/libwebrtc.a and include/;
// the runner passes --vectors <repo>/testvectors/tx-len/vectors.json).
// Exit 0 = every expectation held; 1 = a sender failure (the list is printed); 2 = a harness/control problem.
#ifdef _WIN32
#ifndef NOMINMAX
#define NOMINMAX
#endif
#include <windows.h>
#include <bcrypt.h>
#include <stdlib.h>
#else
#include <openssl/evp.h>
#include <signal.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <unistd.h>
#endif

#include <algorithm>
#include <cctype>
#include <cerrno>
#include <cstdio>
#include <cstring>
#include <fstream>
#include <functional>
#include <map>
#include <memory>
#include <mutex>
#include <optional>
#include <random>
#include <regex>
#include <sstream>
#include <string>
#include <vector>

#include "api/crypto/frame_crypto_transformer.h"
#include "api/frame_transformer_interface.h"
#include "api/make_ref_counted.h"
#include "api/video/video_frame_metadata.h"
#include "common_video/h264/h264_common.h"
#include "modules/rtp_rtcp/source/rtp_video_header.h"
#include "rtc_base/event.h"
#include "rtc_base/logging.h"

namespace {
using Bytes = std::vector<uint8_t>;

// A frame's bytes at the END of a mapping whose next page is PROT_NONE: one byte read past the frame = SIGSEGV.
class Guarded {
 public:
  explicit Guarded(const Bytes& b) : size_(b.size()) {
#ifdef _WIN32
    SYSTEM_INFO si;
    GetSystemInfo(&si);
    const size_t pg = si.dwPageSize;
#else
    const size_t pg = static_cast<size_t>(sysconf(_SC_PAGESIZE));
#endif
    const size_t data_pages = b.size() / pg + 1;
    len_ = (data_pages + 1) * pg;
#ifdef _WIN32
    base_ = static_cast<uint8_t*>(VirtualAlloc(nullptr, len_, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE));
    DWORD old = 0;
    if (!base_ || !VirtualProtect(base_ + data_pages * pg, pg, PAGE_NOACCESS, &old)) {
      std::fprintf(stderr, "harness: guard page failed\n");
      _exit(2);
    }
#else
    void* m = mmap(nullptr, len_, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (m == MAP_FAILED) { std::fprintf(stderr, "harness: mmap failed\n"); _exit(2); }
    base_ = static_cast<uint8_t*>(m);
    if (mprotect(base_ + data_pages * pg, pg, PROT_NONE) != 0) { std::fprintf(stderr, "harness: mprotect failed\n"); _exit(2); }
#endif
    data_ = base_ + data_pages * pg - size_;
    if (size_) std::memcpy(data_, b.data(), size_);
  }
#ifdef _WIN32
  ~Guarded() { VirtualFree(base_, 0, MEM_RELEASE); }
#else
  ~Guarded() { munmap(base_, len_); }
#endif
  Guarded(const Guarded&) = delete;
  Guarded& operator=(const Guarded&) = delete;
  webrtc::ArrayView<const uint8_t> View() const { return webrtc::ArrayView<const uint8_t>(data_, size_); }
  const uint8_t* End() const { return data_ + size_; }

 private:
  uint8_t* base_ = nullptr;
  uint8_t* data_ = nullptr;
  size_t len_ = 0;
  size_t size_ = 0;
};
}  // namespace

namespace webrtc {
// The names are TransformableFrameInterface::Passkey's friends (the upstream test mocks'), the only way to construct
// a frame outside libwebrtc. Not linked from the archive (its tests are not built), so no clash. The data the sender
// reads is GUARDED; what it writes back (SetData) is kept in a plain vector.
class MockTransformableVideoFrame : public TransformableVideoFrameInterface {
 public:
  MockTransformableVideoFrame(const std::vector<uint8_t>& data, VideoCodecType codec, bool key, uint32_t ssrc,
                              uint32_t ts, Direction dir)
      : TransformableVideoFrameInterface(Passkey()), in_(std::make_unique<Guarded>(data)), key_(key), ssrc_(ssrc),
        ts_(ts), dir_(dir) {
    header_.codec = codec;
    header_.frame_type = key ? VideoFrameType::kVideoFrameKey : VideoFrameType::kVideoFrameDelta;
  }
  ArrayView<const uint8_t> GetData() const override {
    return out_ ? ArrayView<const uint8_t>(*out_) : in_->View();
  }
  void SetData(ArrayView<const uint8_t> d) override { out_.emplace(d.begin(), d.end()); }
  uint8_t GetPayloadType() const override { return 96; }
  uint32_t GetSsrc() const override { return ssrc_; }
  uint32_t GetTimestamp() const override { return ts_; }
  void SetRTPTimestamp(uint32_t t) override { ts_ = t; }
  Direction GetDirection() const override { return dir_; }
  std::string GetMimeType() const override { return "video/x"; }
  std::optional<Timestamp> ReceiveTime() const override { return std::nullopt; }
  std::optional<Timestamp> CaptureTime() const override { return std::nullopt; }
  std::optional<TimeDelta> SenderCaptureTimeOffset() const override { return std::nullopt; }
  bool IsKeyFrame() const override { return key_; }
  VideoFrameMetadata Metadata() const override { return VideoFrameMetadata(); }
  void SetMetadata(const VideoFrameMetadata&) override {}
  const RTPVideoHeader& header() const override { return header_; }

 private:
  std::unique_ptr<Guarded> in_;
  std::optional<std::vector<uint8_t>> out_;
  bool key_;
  uint32_t ssrc_, ts_;
  Direction dir_;
  RTPVideoHeader header_;
};
class MockTransformableAudioFrame : public TransformableAudioFrameInterface {
 public:
  MockTransformableAudioFrame(const std::vector<uint8_t>& data, uint32_t ssrc, uint32_t ts, Direction dir)
      : TransformableAudioFrameInterface(Passkey()), in_(std::make_unique<Guarded>(data)), ssrc_(ssrc), ts_(ts),
        dir_(dir) {}
  ArrayView<const uint8_t> GetData() const override {
    return out_ ? ArrayView<const uint8_t>(*out_) : in_->View();
  }
  void SetData(ArrayView<const uint8_t> d) override { out_.emplace(d.begin(), d.end()); }
  uint8_t GetPayloadType() const override { return 111; }
  uint32_t GetSsrc() const override { return ssrc_; }
  uint32_t GetTimestamp() const override { return ts_; }
  void SetRTPTimestamp(uint32_t t) override { ts_ = t; }
  Direction GetDirection() const override { return dir_; }
  std::string GetMimeType() const override { return "audio/opus"; }
  std::optional<Timestamp> ReceiveTime() const override { return std::nullopt; }
  std::optional<Timestamp> CaptureTime() const override { return std::nullopt; }
  std::optional<TimeDelta> SenderCaptureTimeOffset() const override { return std::nullopt; }
  ArrayView<const uint32_t> GetContributingSources() const override { return {}; }
  const std::optional<uint16_t> SequenceNumber() const override { return std::nullopt; }
  std::optional<uint64_t> AbsoluteCaptureTimestamp() const override { return std::nullopt; }
  std::optional<uint8_t> AudioLevel() const override { return std::nullopt; }

 private:
  std::unique_ptr<Guarded> in_;
  std::optional<std::vector<uint8_t>> out_;
  uint32_t ssrc_, ts_;
  Direction dir_;
};
}  // namespace webrtc

namespace {
using webrtc::VideoCodecType;
using Dir = webrtc::TransformableFrameInterface::Direction;

std::mt19937_64 rng(0x7e5d1e9a11ull);
uint8_t R8() { return static_cast<uint8_t>(rng()); }

enum class Kind { kH264, kH265, kVp8, kVp9, kAv1, kAudio };
struct CodecCase {
  const char* name;
  Kind kind;
  VideoCodecType codec;  // unused for audio
  bool nal;              // RBSP-escaped body (H.264/H.265)
};
bool IsAudio(const CodecCase& c) { return c.kind == Kind::kAudio; }

std::unique_ptr<webrtc::TransformableFrameInterface> MakeFrame(const CodecCase& c, const Bytes& data, bool key,
                                                                uint32_t ssrc, uint32_t ts, Dir dir) {
  if (IsAudio(c)) return std::make_unique<webrtc::MockTransformableAudioFrame>(data, ssrc, ts, dir);
  return std::make_unique<webrtc::MockTransformableVideoFrame>(data, c.codec, key, ssrc, ts, dir);
}

// Frames that reached a sink, by RTP timestamp (each frame of a run has its own).
class Sink : public webrtc::TransformedFrameCallback {
 public:
  void OnTransformedFrame(std::unique_ptr<webrtc::TransformableFrameInterface> f) override {
    std::lock_guard<std::mutex> l(mu_);
    auto d = f->GetData();
    if (out_.count(f->GetTimestamp())) dups_++;
    out_[f->GetTimestamp()] = Bytes(d.begin(), d.end());
    if (f->GetTimestamp() == sentinel_) ev_.Set();
  }
  void Expect(uint32_t sentinel) {
    std::lock_guard<std::mutex> l(mu_);
    sentinel_ = sentinel;
    ev_.Reset();
  }
  bool Wait() { return ev_.Wait(webrtc::TimeDelta::Seconds(30)); }
  int TakeDups() {
    std::lock_guard<std::mutex> l(mu_);
    const int d = dups_;
    dups_ = 0;
    return d;
  }
  std::map<uint32_t, Bytes> Take() {
    std::lock_guard<std::mutex> l(mu_);
    auto m = std::move(out_);
    out_.clear();
    return m;
  }

 private:
  std::mutex mu_;
  std::map<uint32_t, Bytes> out_;
  uint32_t sentinel_ = 0;
  int dups_ = 0;
  webrtc::Event ev_;
};

struct Cryptor {
  webrtc::scoped_refptr<webrtc::FrameCryptorTransformer> t;
  webrtc::FrameTransformerInterface* ft = nullptr;  // Transform / Register* are public only on the interface
  webrtc::scoped_refptr<Sink> sink;
};

constexpr uint32_t kSsrc = 0x51c0ffee;
constexpr uint32_t kSentinelTs = 0xfffffff0u;
const char* kPid = "peer";

webrtc::scoped_refptr<webrtc::KeyProvider> MakeKeyProvider(const Bytes& material) {
  webrtc::KeyProviderOptions o;
  o.shared_key = true;
  o.ratchet_salt = Bytes{'s', 'a', 'l', 't'};
  o.ratchet_window_size = 0;
  o.failure_tolerance = -1;
  auto kp = webrtc::make_ref_counted<webrtc::DefaultKeyProviderImpl>(o);
  kp->SetSharedKey(0, material);
  kp->SetSharedKey(3, material);  // the trailer's key index: a frame sealed under slot 3 names 3
  return kp;
}

Cryptor MakeCryptor(webrtc::scoped_refptr<webrtc::KeyProvider> kp, bool audio, int key_index = 0) {
  Cryptor c;
  c.t = webrtc::scoped_refptr<webrtc::FrameCryptorTransformer>(new webrtc::FrameCryptorTransformer(
      nullptr, kPid,
      audio ? webrtc::FrameCryptorTransformer::MediaType::kAudioFrame
            : webrtc::FrameCryptorTransformer::MediaType::kVideoFrame,
      webrtc::FrameCryptorTransformer::Algorithm::kAesGcm, kp));
  c.sink = webrtc::make_ref_counted<Sink>();
  c.ft = c.t.get();
  c.ft->RegisterTransformedFrameCallback(c.sink);
  c.ft->RegisterTransformedFrameSinkCallback(c.sink, kSsrc);
  c.t->SetKeyIndex(key_index);
  c.t->SetEnabled(true);
  return c;
}

std::vector<std::string> g_fail;  // per child; the child's exit code carries the verdict to the parent
int g_harness = 0;

std::string Hex(const Bytes& b, size_t max = 12) {
  std::string s;
  char t[4];
  for (size_t i = 0; i < b.size() && i < max; i++) { std::snprintf(t, sizeof t, "%02x ", b[i]); s += t; }
  if (b.size() > max) s += "...";
  else if (!s.empty()) s.pop_back();
  return s;
}

// --- frames ---
struct TxFrame {
  Bytes data;
  bool key = false;
  size_t header = 0;  // the codec's TRUE clear header length (get_unencrypted_bytes' rule, before any uint8_t narrowing)
};
const Bytes kSps = {0, 0, 0, 1, 0x67, 0x42, 0xc0, 0x1f, 0x8c, 0x8d, 0x40};  // no zero runs
const Bytes kPps = {0, 0, 0, 1, 0x68, 0xce, 0x3c, 0x80};
const Bytes kVps265 = {0, 0, 0, 1, 0x40, 0x01, 0x0c, 0x01, 0xff, 0xff};
const Bytes kSps265 = {0, 0, 0, 1, 0x42, 0x01, 0x01, 0x01, 0x60};
const Bytes kPps265 = {0, 0, 0, 1, 0x44, 0x01, 0xc1, 0x72};
Bytes Cat(std::initializer_list<Bytes> parts) {
  Bytes o;
  for (const auto& p : parts) o.insert(o.end(), p.begin(), p.end());
  return o;
}
// body bytes with no zero (no start code, no emulation sequence can form inside them)
Bytes Body(size_t n) {
  Bytes b(n);
  for (auto& x : b) x = static_cast<uint8_t>(1 + R8() % 255);
  return b;
}
// The slice NAL's header byte(s) per codec; first byte of the slice data after them.
Bytes SliceNal(Kind k, bool key) {
  if (k == Kind::kH265) return {0, 0, 0, 1, static_cast<uint8_t>((key ? 19 : 1) << 1), 0x01};
  return {0, 0, 0, 1, static_cast<uint8_t>(key ? 0x65 : 0x41)};
}
// header = slice NAL payload offset + 2 (H.264: NAL header byte + 1; H.265: its 2-byte NAL header)
TxFrame NalFrame(Kind k, bool key, bool params, size_t body) {
  Bytes pre;
  if (params && k == Kind::kH264) pre = Cat({kSps, kPps});
  if (params && k == Kind::kH265) pre = Cat({kVps265, kSps265, kPps265});
  TxFrame f;
  f.key = key;
  f.data = Cat({pre, SliceNal(k, key)});
  f.header = pre.size() + 4 + 2;
  if (k == Kind::kH264) {
    // H.264: the clear header ends one byte INTO the slice data (offset + 2 = NAL header + 1 byte)
    if (body) { Bytes b = Body(body); f.data.insert(f.data.end(), b.begin(), b.end()); }
  } else {
    Bytes b = Body(body);
    f.data.insert(f.data.end(), b.begin(), b.end());
  }
  return f;
}
// A frame of `extra` bytes past the codec's clear header (VP9/AV1 have none: extra >= 1, an empty frame isn't sealed).
TxFrame Plain(const CodecCase& cc, bool key, size_t extra) {
  TxFrame f;
  f.key = key;
  switch (cc.kind) {
    case Kind::kH264: {
      f = NalFrame(Kind::kH264, key, key, 0);  // [SPS PPS] + slice NAL header; header = size + 1
      Bytes b = Body(1 + extra);
      f.data.insert(f.data.end(), b.begin(), b.end());
      return f;
    }
    case Kind::kH265:
      return NalFrame(Kind::kH265, key, key, extra);
    case Kind::kVp8:
      f.header = key ? 10 : 3;
      f.data = Body(f.header + extra);
      f.data[0] = key ? 0x10 : 0x11;
      return f;
    case Kind::kVp9:
      f.header = 0;
      f.data = Body(extra);
      if (!f.data.empty()) f.data[0] = key ? 0x82 : 0x86;
      return f;
    case Kind::kAv1:
      f.header = 0;
      f.data = Body(extra);
      if (!f.data.empty()) f.data[0] = 0x32;
      return f;
    case Kind::kAudio:
      f.header = 1;
      f.data = Body(1 + extra);
      f.data[0] = 0x78;  // the Opus TOC byte (clear)
      return f;
  }
  return f;
}
std::vector<TxFrame> ShortFrames(const CodecCase& cc) {
  std::vector<TxFrame> v;
  auto add = [&](Bytes b, bool key, size_t header) { v.push_back({std::move(b), key, header}); };
  switch (cc.kind) {
    case Kind::kH264:
      add({0, 0, 1, 0x65}, true, 5);  // the review's example: header 5 of 4 bytes
      add({0, 0, 1, 0x41}, false, 5);
      add({0, 0, 1, 0x25}, true, 5);   // IDR, nal_ref_idc 1
      add({0, 0, 1, 0x01}, false, 5);  // non-IDR slice, nal_ref_idc 0
      add({0, 0, 0, 1, 0x65}, true, 6);
      add({0, 0, 0, 1, 0x41}, false, 6);
      add(Cat({kSps, kPps, {0, 0, 0, 1, 0x65}}), true, kSps.size() + kPps.size() + 6);
      break;
    case Kind::kH265:
      add({0, 0, 1, 0x26}, true, 5);   // IDR_W_RADL (19), the 2-byte NAL header cut after its first byte
      add({0, 0, 1, 0x02}, false, 5);  // TRAIL_R (1)
      add({0, 0, 0, 1, 0x26}, true, 6);
      add({0, 0, 0, 1, 0x02}, false, 6);
      add(Cat({kVps265, kSps265, kPps265, {0, 0, 0, 1, 0x26}}), true,
          kVps265.size() + kSps265.size() + kPps265.size() + 6);
      break;
    case Kind::kVp8:
      for (size_t n = 1; n < 10; n++) { Bytes b = Body(n); b[0] = 0x10; add(b, true, 10); }
      for (size_t n = 1; n < 3; n++) { Bytes b = Body(n); b[0] = 0x11; add(b, false, 3); }
      break;
    default:
      break;
  }
  return v;
}

// Sends frames (ts = base + 3000 i) then a valid sentinel frame through the sender; returns what came out, by ts
// (the sentinel included).
std::map<uint32_t, Bytes> Send(Cryptor& tx, const CodecCase& cc, const std::vector<TxFrame>& in, uint32_t base,
                               const TxFrame& sentinel) {
  tx.sink->Expect(kSentinelTs);
  for (size_t i = 0; i < in.size(); i++)
    tx.ft->Transform(MakeFrame(cc, in[i].data, in[i].key, kSsrc, base + 3000 * static_cast<uint32_t>(i), Dir::kSender));
  tx.ft->Transform(MakeFrame(cc, sentinel.data, sentinel.key, kSsrc, kSentinelTs, Dir::kSender));
  if (!tx.sink->Wait()) { std::printf("  harness: the sender's sentinel frame never came out\n"); g_harness++; }
  if (const int d = tx.sink->TakeDups()) g_fail.push_back(std::string(cc.name) + ": " + std::to_string(d) + " frames sent twice");
  return tx.sink->Take();
}
// Feeds sealed frames (ts -> bytes, key) and the sealed sentinel to a receiver; returns what it delivered (no sentinel).
std::map<uint32_t, Bytes> Receive(Cryptor& rx, const CodecCase& cc, const std::map<uint32_t, std::pair<Bytes, bool>>& in,
                                  const Bytes& sealed_sentinel) {
  rx.sink->Expect(kSentinelTs);
  for (const auto& [ts, fk] : in) rx.ft->Transform(MakeFrame(cc, fk.first, fk.second, kSsrc, ts, Dir::kReceiver));
  rx.ft->Transform(MakeFrame(cc, sealed_sentinel, true, kSsrc, kSentinelTs, Dir::kReceiver));
  if (!rx.sink->Wait()) { std::printf("  harness: the receiver's sentinel frame never came back\n"); g_harness++; }
  if (const int d = rx.sink->TakeDups()) g_fail.push_back(std::string(cc.name) + ": " + std::to_string(d) + " frames delivered twice");
  auto got = rx.sink->Take();
  got.erase(kSentinelTs);
  return got;
}

// Does `hay` contain a run of `n` bytes of `plain` starting at `from` (plaintext in the clear)?
bool CarriesPlainRun(const Bytes& hay, const Bytes& plain, size_t from, size_t n) {
  if (from + n > plain.size() || n == 0) return false;
  return std::search(hay.begin(), hay.end(), plain.begin() + from, plain.begin() + from + n) != hay.end();
}

// AES-GCM open independent of the archive (system OpenSSL; Windows: CNG): ct_tag = ciphertext || 16-byte tag.
#ifdef _WIN32
std::optional<Bytes> GcmOpen(const Bytes& key, const Bytes& iv, const Bytes& aad, const Bytes& ct_tag) {
  if ((key.size() != 16 && key.size() != 32) || ct_tag.size() < 16) return std::nullopt;
  const size_t n = ct_tag.size() - 16;
  Bytes k(key), nonce(iv), ad(aad), ct(ct_tag.begin(), ct_tag.end() - 16), tag(ct_tag.end() - 16, ct_tag.end());
  Bytes out(n + 1, 0);
  BCRYPT_ALG_HANDLE alg = nullptr;
  BCRYPT_KEY_HANDLE kh = nullptr;
  bool ok = BCRYPT_SUCCESS(BCryptOpenAlgorithmProvider(&alg, BCRYPT_AES_ALGORITHM, nullptr, 0)) &&
            BCRYPT_SUCCESS(BCryptSetProperty(alg, BCRYPT_CHAINING_MODE, (PUCHAR)BCRYPT_CHAIN_MODE_GCM,
                                             sizeof(BCRYPT_CHAIN_MODE_GCM), 0)) &&
            BCRYPT_SUCCESS(BCryptGenerateSymmetricKey(alg, &kh, nullptr, 0, k.data(), static_cast<ULONG>(k.size()), 0));
  if (ok) {
    BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO ai;
    BCRYPT_INIT_AUTH_MODE_INFO(ai);
    ai.pbNonce = nonce.data();
    ai.cbNonce = static_cast<ULONG>(nonce.size());
    ai.pbAuthData = ad.empty() ? nullptr : ad.data();
    ai.cbAuthData = static_cast<ULONG>(ad.size());
    ai.pbTag = tag.data();
    ai.cbTag = 16;
    ULONG got = 0;
    ok = BCRYPT_SUCCESS(BCryptDecrypt(kh, n ? ct.data() : nullptr, static_cast<ULONG>(n), &ai, nullptr, 0,
                                      n ? out.data() : nullptr, static_cast<ULONG>(n), &got, 0)) && got == n;
  }
  if (kh) BCryptDestroyKey(kh);
  if (alg) BCryptCloseAlgorithmProvider(alg, 0);
  if (!ok) return std::nullopt;
  out.resize(n);
  return out;
}
#else
std::optional<Bytes> GcmOpen(const Bytes& key, const Bytes& iv, const Bytes& aad, const Bytes& ct_tag) {
  const EVP_CIPHER* ci = key.size() == 16 ? EVP_aes_128_gcm() : key.size() == 32 ? EVP_aes_256_gcm() : nullptr;
  if (!ci || ct_tag.size() < 16) return std::nullopt;
  const size_t n = ct_tag.size() - 16;
  Bytes out(n + 16, 0), tag(ct_tag.end() - 16, ct_tag.end());
  EVP_CIPHER_CTX* x = EVP_CIPHER_CTX_new();
  int len = 0;
  bool ok = EVP_DecryptInit_ex(x, ci, nullptr, nullptr, nullptr) == 1 &&
            EVP_CIPHER_CTX_ctrl(x, EVP_CTRL_GCM_SET_IVLEN, static_cast<int>(iv.size()), nullptr) == 1 &&
            EVP_DecryptInit_ex(x, nullptr, nullptr, key.data(), iv.data()) == 1 &&
            (aad.empty() || EVP_DecryptUpdate(x, nullptr, &len, aad.data(), static_cast<int>(aad.size())) == 1) &&
            (n == 0 || EVP_DecryptUpdate(x, out.data(), &len, ct_tag.data(), static_cast<int>(n)) == 1) &&
            EVP_CIPHER_CTX_ctrl(x, EVP_CTRL_GCM_SET_TAG, 16, tag.data()) == 1 &&
            EVP_DecryptFinal_ex(x, out.data() + n, &len) == 1;
  EVP_CIPHER_CTX_free(x);
  if (!ok) return std::nullopt;
  out.resize(n);
  return out;
}
#endif
Bytes g_key;  // the transformer's derived key for slot 0 (read in main, before any child)

// One sealed frame through a fresh receiver cryptor (it doubles as that run's sentinel): the bytes it delivers.
std::optional<Bytes> DecryptAlone(webrtc::scoped_refptr<webrtc::KeyProvider> kp, const CodecCase& cc, const Bytes& sealed,
                                  bool key) {
  Cryptor rx = MakeCryptor(kp, IsAudio(cc));
  rx.sink->Expect(kSentinelTs);
  rx.ft->Transform(MakeFrame(cc, sealed, key, kSsrc, kSentinelTs, Dir::kReceiver));
  if (!rx.sink->Wait()) return std::nullopt;
  auto got = rx.sink->Take();
  auto it = got.find(kSentinelTs);
  if (it == got.end()) return std::nullopt;
  return it->second;
}

// The checks on one SEALED frame from the sender. `wire_header` = the clear header the sender is expected to keep.
void CheckSealed(const std::string& label, const CodecCase& cc, const TxFrame& in, const Bytes& out, size_t wire_header,
                 int key_index) {
  const size_t sealed = in.data.size() - wire_header + 16 + 12 + 2;  // body + tag + IV + trailer
  if (out.size() < 2 || out[out.size() - 2] != 12 || out[out.size() - 1] != key_index) {
    g_fail.push_back(label + ": trailer is not [12, " + std::to_string(key_index) + "] (tail " +
                     Hex(Bytes(out.end() - std::min<size_t>(out.size(), 4), out.end())) + ")");
    return;
  }
  if (cc.kind == Kind::kAv1) {
    const Bytes wrap = in.key ? Bytes{0x0a, 0x00, 0x30} : Bytes{0x30};
    if (out.size() != wrap.size() + sealed || !std::equal(wrap.begin(), wrap.end(), out.begin()))
      g_fail.push_back(label + ": AV1 wire form wrong (" + std::to_string(out.size()) + " bytes, starts " + Hex(out, 4) + ")");
  } else {
    if (out.size() < wire_header || !std::equal(in.data.begin(), in.data.begin() + wire_header, out.begin()))
      g_fail.push_back(label + ": clear header not kept as is");
    if (cc.nal ? out.size() < wire_header + sealed : out.size() != wire_header + sealed)
      g_fail.push_back(label + ": sealed length " + std::to_string(out.size()) + ", expected " + (cc.nal ? ">= " : "") +
                       std::to_string(wire_header + sealed));
  }
  // never sent as is: no 16-byte run of the body past the clear header appears in the output
  if (in.data.size() >= in.header + 16 && CarriesPlainRun(out, in.data, in.header, 16))
    g_fail.push_back(label + ": PLAINTEXT of the body in the sealed frame");
}

// ---- SHORT ----
int RunShort(const CodecCase& cc, webrtc::scoped_refptr<webrtc::KeyProvider> kp) {
  const auto frames = ShortFrames(cc);
  Cryptor tx = MakeCryptor(kp, IsAudio(cc));
  const TxFrame sentinel = Plain(cc, true, 200);
  auto got = Send(tx, cc, frames, 1000, sentinel);
  const bool sentinel_ok = got.count(kSentinelTs) == 1;
  const Bytes sealed_sentinel = sentinel_ok ? got[kSentinelTs] : Bytes();
  got.erase(kSentinelTs);
  std::printf("%-6s SHORT      sent %3zu frames shorter than their clear header        delivered %3zu (must be 0)%s\n",
              cc.name, frames.size(), got.size(), sentinel_ok ? "" : "  SENTINEL MISSING");
  for (const auto& [ts, b] : got) {
    const size_t i = (ts - 1000) / 3000;
    g_fail.push_back(std::string(cc.name) + " SHORT: [" + Hex(frames[i].data) + "] was delivered (" +
                     std::to_string(b.size()) + " bytes" + (b == frames[i].data ? ", AS IS" : "") + ")");
  }
  if (!sentinel_ok) g_fail.push_back(std::string(cc.name) + " SHORT: the valid frame after them was not sealed");
  else {
    CheckSealed(std::string(cc.name) + " SHORT sentinel", cc, sentinel, sealed_sentinel, sentinel.header, 0);
    auto back = DecryptAlone(kp, cc, sealed_sentinel, sentinel.key);
    if (!back || *back != sentinel.data)
      g_fail.push_back(std::string(cc.name) + " SHORT: the valid frame after them does not decrypt byte-identical");
  }
  return g_harness ? 2 : g_fail.empty() ? 0 : 1;
}

// ---- BOUNDARY + OVERSIZED: seal, check, decrypt back ----
int RunRoundTrip(const CodecCase& cc, webrtc::scoped_refptr<webrtc::KeyProvider> kp, const std::string& set,
                 const std::vector<TxFrame>& frames, int key_index) {
  Cryptor tx = MakeCryptor(kp, IsAudio(cc), key_index);
  const TxFrame sentinel = Plain(cc, true, 200);
  auto out = Send(tx, cc, frames, 1000, sentinel);
  const std::string base = std::string(cc.name) + " " + set + " (key index " + std::to_string(key_index) + ")";
  if (!out.count(kSentinelTs)) { g_fail.push_back(base + ": sentinel not sealed"); return 1; }
  const Bytes sealed_sentinel = out[kSentinelTs];
  std::map<uint32_t, std::pair<Bytes, bool>> to_rx;
  size_t sealed = 0;
  for (size_t i = 0; i < frames.size(); i++) {
    const uint32_t ts = 1000 + 3000 * static_cast<uint32_t>(i);
    const std::string label = base + " frame " + std::to_string(i) + " (" + std::to_string(frames[i].data.size()) +
                              " bytes, header " + std::to_string(frames[i].header) + ")";
    auto it = out.find(ts);
    if (it == out.end()) { g_fail.push_back(label + ": DROPPED (a frame at least as long as its header must be sealed)"); continue; }
    sealed++;
    CheckSealed(label, cc, frames[i], it->second, frames[i].header, key_index);
    to_rx[ts] = {it->second, frames[i].key};
  }
  Cryptor rx = MakeCryptor(kp, IsAudio(cc));
  auto back = Receive(rx, cc, to_rx, sealed_sentinel);
  size_t same = 0;
  for (const auto& [ts, fk] : to_rx) {
    const size_t i = (ts - 1000) / 3000;
    auto it = back.find(ts);
    if (it == back.end()) g_fail.push_back(base + " frame " + std::to_string(i) + ": not decrypted by the receiver");
    else if (it->second != frames[i].data) g_fail.push_back(base + " frame " + std::to_string(i) + ": decrypted to OTHER bytes");
    else same++;
  }
  std::printf("%-6s %-9s key index %d: sent %3zu  sealed %3zu  decrypted byte-identical %3zu\n", cc.name, set.c_str(),
              key_index, frames.size(), sealed, same);
  return g_harness ? 2 : g_fail.empty() ? 0 : 1;
}
std::vector<TxFrame> BoundaryFrames(const CodecCase& cc) {
  std::vector<TxFrame> v;
  const bool no_header = cc.kind == Kind::kVp9 || cc.kind == Kind::kAv1;
  for (int key = 1; key >= 0; key--)
    for (size_t extra = 0; extra <= 2; extra++) {
      TxFrame f = Plain(cc, key != 0, no_header ? extra + 1 : extra);
      if (cc.kind == Kind::kH264) {  // header exactly: [SPS PPS] + slice NAL header + 1 byte (+ extra)
        f = NalFrame(Kind::kH264, key != 0, key != 0, 1 + extra);
      }
      if (!no_header && f.data.size() != f.header + extra) { std::printf("  harness: boundary frame size\n"); g_harness++; }
      v.push_back(std::move(f));
      if (cc.nal) {  // and the bare slice NAL with a 3-byte start code, no parameter sets
        TxFrame g;
        g.key = key != 0;
        g.data = cc.kind == Kind::kH264 ? Bytes{0, 0, 1, static_cast<uint8_t>(key ? 0x65 : 0x41), 0xb8}
                                        : Bytes{0, 0, 1, static_cast<uint8_t>((key ? 19 : 1) << 1), 0x01};
        g.header = 5;
        Bytes b = Body(extra);
        g.data.insert(g.data.end(), b.begin(), b.end());
        v.push_back(std::move(g));
      }
    }
  return v;
}
std::vector<TxFrame> OversizedFrames(const CodecCase& cc) {
  std::vector<TxFrame> v;
  for (size_t mib : {1, 4, 16}) v.push_back(Plain(cc, true, (mib << 20) - 64));
  v.push_back(Plain(cc, false, (4u << 20) - 64));
  return v;
}

// ---- H.264 WIDE HEADER: the slice NAL past offset 250 (the uint8_t header length) ----
int RunWideHeader(const CodecCase& cc, webrtc::scoped_refptr<webrtc::KeyProvider> kp) {
  Cryptor tx = MakeCryptor(kp, false);
  std::vector<TxFrame> frames;
  std::vector<size_t> offs;
  for (size_t off : {251, 252, 253, 254, 255, 256, 257, 300, 600}) {
    // SPS + PPS + an SEI NAL padded so the IDR slice NAL's payload starts at `off` (4-byte start codes)
    const size_t sei_body = off - kSps.size() - kPps.size() - 5 - 4;
    TxFrame f;
    f.key = true;
    f.data = Cat({kSps, kPps, {0, 0, 0, 1, 0x06}, Bytes(sei_body, 0x55), {0, 0, 0, 1, 0x65}, Body(400)});
    f.header = off + 2;
    if (f.data[off - 1] != 0x01 || f.data[off] != 0x65) { std::printf("  harness: wide-header layout\n"); return 2; }
    frames.push_back(std::move(f));
    offs.push_back(off);
  }
  const TxFrame sentinel = Plain(cc, true, 200);
  auto out = Send(tx, cc, frames, 1000, sentinel);
  if (!out.count(kSentinelTs)) { g_fail.push_back("H264 WIDE: sentinel not sealed"); return 1; }
  const Bytes sealed_sentinel = out[kSentinelTs];
  std::map<uint32_t, std::pair<Bytes, bool>> to_rx;
  for (size_t i = 0; i < frames.size(); i++) {
    const uint32_t ts = 1000 + 3000 * static_cast<uint32_t>(i);
    auto it = out.find(ts);
    if (it != out.end()) to_rx[ts] = {it->second, true};
  }
  Cryptor rx = MakeCryptor(kp, false);
  auto back = Receive(rx, cc, to_rx, sealed_sentinel);
  for (size_t i = 0; i < frames.size(); i++) {
    const uint32_t ts = 1000 + 3000 * static_cast<uint32_t>(i);
    const std::string label = "H264 WIDE slice NAL at offset " + std::to_string(offs[i]) + " (header " +
                              std::to_string(frames[i].header) + ")";
    auto it = out.find(ts);
    const bool fits = frames[i].header <= 255;
    if (it == out.end()) {
      std::printf("%-58s dropped by the sender\n", label.c_str());
      if (fits) g_fail.push_back(label + ": DROPPED");
      continue;
    }
    const Bytes& o = it->second;
    size_t clear = 0;
    while (clear < o.size() && clear < frames[i].data.size() && o[clear] == frames[i].data[clear]) clear++;
    const bool dec = back.count(ts) && back[ts] == frames[i].data;
    std::printf("%-58s sealed, %3zu clear bytes in front, opens independently, receiver: %s\n", label.c_str(), clear,
                dec ? "decrypts byte-identical" : fits ? "NOT decrypted" : "not decrypted (INFO: the header length wraps)");
    if (o.size() < 2 || o[o.size() - 2] != 12 || o[o.size() - 1] != 0) g_fail.push_back(label + ": trailer wrong");
    if (CarriesPlainRun(o, frames[i].data, frames[i].header, 16)) g_fail.push_back(label + ": PLAINTEXT of the slice body");
    // independent open: AAD = the wrapped clear header N = header mod 256, body = ParseRbsp(rest) = ct || tag || IV || trailer
    const size_t n_clear = frames[i].header & 0xff;
    bool opened = false;
    if (o.size() > n_clear && std::equal(frames[i].data.begin(), frames[i].data.begin() + n_clear, o.begin())) {
      const std::vector<uint8_t> body = webrtc::H264::ParseRbsp(o.data() + n_clear, o.size() - n_clear);
      if (body.size() >= 16 + 12 + 2) {
        const Bytes iv(body.end() - 14, body.end() - 2), ct(body.begin(), body.end() - 14);
        auto pt = GcmOpen(g_key, iv, Bytes(o.begin(), o.begin() + n_clear), ct);
        opened = pt && *pt == Bytes(frames[i].data.begin() + n_clear, frames[i].data.end());
      }
    }
    if (!opened) g_fail.push_back(label + ": does NOT open as clear header (" + std::to_string(n_clear) + " bytes) + a sealed rest");
    if (fits) {
      CheckSealed(label, cc, frames[i], o, frames[i].header, 0);
      if (!dec) g_fail.push_back(label + ": not decrypted byte-identical");
    }
  }
  return g_harness ? 2 : g_fail.empty() ? 0 : 1;
}

// ---- VECTORS (testvectors/tx-len/vectors.json, shared with tinhat-web) ----
struct Vector {
  std::string codec, why, hex;
  bool key = false, drop = false;
  Bytes data;
  size_t header = 0;
};
std::vector<Vector> LoadVectors(const std::string& path, std::string* err) {
  std::ifstream f(path);
  if (!f) { *err = "cannot read " + path; return {}; }
  std::stringstream ss;
  ss << f.rdbuf();
  const std::string s = ss.str();
  const size_t at = s.find("\"cases\"");
  if (at == std::string::npos || s.find("\"stamp\": \"tinhat-tx-len-v1\"") == std::string::npos) { *err = "not the tx-len vectors"; return {}; }
  std::vector<Vector> out;
  const std::regex obj(R"(\{[^{}]*\})");
  for (auto it = std::sregex_iterator(s.begin() + at, s.end(), obj); it != std::sregex_iterator(); ++it) {
    const std::string o = it->str();
    auto field = [&](const std::string& k) -> std::string {
      std::smatch m;
      if (!std::regex_search(o, m, std::regex("\"" + k + R"re("\s*:\s*(?:"([^"]*)"|(true|false|[0-9]+)))re"))) return "";
      return m[1].matched ? m[1].str() : m[2].str();
    };
    Vector v;
    v.codec = field("codec"); v.hex = field("hex"); v.why = field("why");
    v.key = field("key") == "true";
    v.drop = field("expect") == "drop";
    const std::string h = field("header"), len = field("len");
    if (v.codec.empty() || h.empty() || len.empty() || v.hex.size() % 2 || (field("expect") != "drop" && field("expect") != "seal")) {
      *err = "malformed case: " + o.substr(0, 80);
      return {};
    }
    v.header = std::stoul(h);
    for (size_t i = 0; i < v.hex.size(); i += 2) v.data.push_back(static_cast<uint8_t>(std::stoul(v.hex.substr(i, 2), nullptr, 16)));
    if (v.data.size() != std::stoul(len) || (v.data.size() < v.header) != v.drop) { *err = "inconsistent case " + v.hex; return {}; }
    out.push_back(std::move(v));
  }
  // the whole document, not just the objects a regex finds: one parsed case per "codec" key, and nothing after the
  // last case but the closing "]" and "}" (a truncated or cut file is refused, never a smaller PASS)
  size_t keys = 0;
  for (size_t p = s.find("\"codec\"", at); p != std::string::npos; p = s.find("\"codec\"", p + 1)) keys++;
  const size_t last = s.rfind('}', s.find_last_not_of(" \t\r\n}") == std::string::npos ? 0 : s.find_last_not_of(" \t\r\n}"));
  const std::string tail = s.substr(s.find_last_not_of(" \t\r\n") == std::string::npos ? 0 : last + 1);
  std::string squeezed;
  for (char ch : tail) if (!std::isspace(static_cast<unsigned char>(ch))) squeezed += ch;
  if (out.empty()) *err = "no cases";
  else if (keys != out.size()) { *err = "case count mismatch (" + std::to_string(keys) + " codec keys, " + std::to_string(out.size()) + " cases)"; out.clear(); }
  else if (squeezed != "]}") { *err = "the document does not end after its last case (truncated?)"; out.clear(); }
  return out;
}
int RunVector(const CodecCase& cc, const Vector& v, webrtc::scoped_refptr<webrtc::KeyProvider> kp) {
  Cryptor tx = MakeCryptor(kp, IsAudio(cc));
  TxFrame f;
  f.data = v.data; f.key = v.key; f.header = v.header;
  const TxFrame sentinel = Plain(cc, true, 200);
  auto out = Send(tx, cc, {f}, 1000, sentinel);
  const std::string label = std::string(cc.name) + " vector [" + Hex(v.data) + "] (" + std::to_string(v.data.size()) +
                            " bytes, header " + std::to_string(v.header) + ", " + (v.drop ? "drop" : "seal") + ")";
  auto s = out.find(kSentinelTs);
  if (s == out.end()) { g_fail.push_back(label + ": the next valid frame was not sealed"); return 1; }
  auto sb = DecryptAlone(kp, cc, s->second, true);
  if (!sb || *sb != sentinel.data) g_fail.push_back(label + ": the next valid frame does not decrypt byte-identical");
  auto it = out.find(1000);
  if (v.drop) {
    if (it != out.end()) g_fail.push_back(label + ": DELIVERED" + (it->second == v.data ? " AS IS" : ""));
  } else if (it == out.end()) {
    g_fail.push_back(label + ": DROPPED");
  } else {
    CheckSealed(label, cc, f, it->second, v.header, 0);
    auto back = DecryptAlone(kp, cc, it->second, v.key);
    if (!back || *back != v.data) g_fail.push_back(label + ": does not decrypt byte-identical");
  }
  std::printf("%-6s VECTOR %-34s %-5s %s\n", cc.name, ("[" + Hex(v.data, 10) + "]").c_str(), v.drop ? "drop" : "seal",
              g_fail.empty() ? "ok" : "FAILED");
  return g_harness ? 2 : g_fail.empty() ? 0 : 1;
}

// ---- child runners: one job per set, each in its own process ----
struct Job {
  std::string label;
  std::function<int()> fn;
  bool expect_fault = false;  // the guard control: the child MUST die of an access fault
};
int g_parent_fail = 0, g_parent_harness = 0;
std::vector<std::string> g_parent_failed;
// the child's side: run the job, print its failures, return its exit code
int RunHere(const Job& j) {
  const int r = j.fn();
  for (const auto& f : g_fail) std::printf("  FAIL %s\n", f.c_str());
  std::fflush(stdout);
  return r;
}
// the parent's verdict on how a child ended
void Verdict(const Job& j, bool died, bool fault, bool hung, int code, const std::string& how) {
  if (j.expect_fault) {
    if (died && fault) std::printf("control: reading one byte past a guarded frame faults (%s)\n", how.c_str());
    else { std::printf("CONTROL BROKEN: reading past a guarded frame did not fault (%s)\n", how.c_str()); g_parent_harness++; }
    return;
  }
  if (died) {
    std::printf("%-58s the sender DIED (%s%s%s)\n", j.label.c_str(), how.c_str(), hung ? ", hung" : "",
                fault ? ", read past the frame" : "");
    g_parent_failed.push_back(j.label + ": the sender died (" + how + ")");
    g_parent_fail++;
  } else if (code == 1) {
    g_parent_failed.push_back(j.label + ": expectation failed (see above)");
    g_parent_fail++;
  } else if (code != 0) {
    std::printf("%-58s harness problem (exit %d)\n", j.label.c_str(), code);
    g_parent_harness++;
  }
}
#ifdef _WIN32
std::string g_self;
std::vector<std::string> g_child_args;
void RunJob(size_t k, const Job& j) {
  std::fflush(stdout);
  std::string cmd = "\"" + g_self + "\" --job " + std::to_string(k);
  for (const auto& a : g_child_args) cmd += " \"" + a + "\"";
  STARTUPINFOA si{};
  si.cb = sizeof si;
  si.dwFlags = STARTF_USESTDHANDLES;
  si.hStdInput = GetStdHandle(STD_INPUT_HANDLE);
  si.hStdOutput = GetStdHandle(STD_OUTPUT_HANDLE);
  si.hStdError = GetStdHandle(STD_ERROR_HANDLE);
  for (HANDLE h : {si.hStdOutput, si.hStdError}) SetHandleInformation(h, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT);
  PROCESS_INFORMATION pi{};
  if (!CreateProcessA(nullptr, cmd.data(), nullptr, nullptr, TRUE, 0, nullptr, nullptr, &si, &pi)) {
    std::printf("harness: CreateProcess failed (%lu)\n", GetLastError());
    g_parent_harness++;
    return;
  }
  const bool hung = WaitForSingleObject(pi.hProcess, j.expect_fault ? 30000 : 300000) == WAIT_TIMEOUT;
  if (hung) TerminateProcess(pi.hProcess, 0xDEAD);
  WaitForSingleObject(pi.hProcess, INFINITE);
  DWORD code = 0;
  GetExitCodeProcess(pi.hProcess, &code);
  CloseHandle(pi.hThread);
  CloseHandle(pi.hProcess);
  char how[48];
  std::snprintf(how, sizeof how, "exit 0x%08lx", static_cast<unsigned long>(code));
  // 0, 1, 2 = the job's own verdicts; anything else = an access violation (0xC0000005), abort (3), fast-fail, ...
  const bool died = hung || (code != 0 && code != 1 && code != 2);
  Verdict(j, died, code == 0xC0000005, hung, static_cast<int>(code), how);
}
#else
void RunJob(size_t, const Job& j) {
  std::fflush(stdout);
  const pid_t pid = fork();
  if (pid < 0) { std::printf("harness: fork failed (%s)\n", std::strerror(errno)); g_parent_harness++; return; }
  if (pid == 0) {
    alarm(j.expect_fault ? 30 : 300);  // a hang is a failure, not a stuck test
    _exit(RunHere(j));
  }
  int st = 0;
  pid_t w;
  do { w = waitpid(pid, &st, 0); } while (w < 0 && errno == EINTR);
  if (w != pid) { std::printf("harness: waitpid failed\n"); g_parent_harness++; return; }
  const bool died = WIFSIGNALED(st);
  const int sig = died ? WTERMSIG(st) : 0;
  Verdict(j, died, sig == SIGSEGV, sig == SIGALRM, WIFEXITED(st) ? WEXITSTATUS(st) : -1,
          died ? "signal " + std::to_string(sig) : "exit " + std::to_string(WIFEXITED(st) ? WEXITSTATUS(st) : -1));
}
#endif
// The guard control's body: reading one byte past a guarded frame must fault (exit 0 = it did NOT).
int GuardControl() {
  Guarded g(Bytes{0, 0, 1, 0x65});
  volatile uint8_t x = g.End()[-1];  // the last byte: fine
  x = g.End()[0];                    // one past: must fault
  (void)x;
  return 0;
}
}  // namespace

int main(int argc, char** argv) {
  bool verbose = false;
  std::string vec_path;
  long job = -1;  // >= 0: this process is a child running that one job
  for (int i = 1; i < argc; i++) {
    const std::string a = argv[i];
    if (a == "-v") verbose = true;
    else if (a == "--vectors" && i + 1 < argc) vec_path = argv[++i];
    else if (a == "--job" && i + 1 < argc) job = std::stol(argv[++i]);
  }
#ifdef _WIN32
  if (job >= 0) {  // a dying child must not wait on a crash dialog
    SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX);
    _set_abort_behavior(0, _WRITE_ABORT_MSG | _CALL_REPORTFAULT);
  }
  char self[MAX_PATH];
  GetModuleFileNameA(nullptr, self, MAX_PATH);
  g_self = self;
  g_child_args = {"--vectors", vec_path};
  if (verbose) g_child_args.push_back("-v");
#endif
  const bool parent = job < 0;
  std::string vec_err;
  const auto vectors = vec_path.empty() ? std::vector<Vector>() : LoadVectors(vec_path, &vec_err);
  if (vectors.empty()) {
    std::printf("harness: shared vectors: %s\n", vec_path.empty() ? "no --vectors given" : vec_err.c_str());
    return 2;
  }
  if (parent) std::printf("shared vectors: %zu cases from %s\n", vectors.size(), vec_path.c_str());
  webrtc::LogMessage::LogToDebug(verbose ? webrtc::LS_INFO : webrtc::LS_NONE);

  Bytes material(32);
  for (auto& x : material) x = R8();
  auto kp = MakeKeyProvider(material);
  g_key = kp->GetSharedKey(kPid)->GetKeySet(0)->encryption_key;
  if (g_key.empty()) { std::printf("harness: no derived key\n"); return 2; }

  std::vector<CodecCase> codecs = {{"H264", Kind::kH264, webrtc::kVideoCodecH264, true},
#ifdef RTC_ENABLE_H265
                                   {"H265", Kind::kH265, webrtc::kVideoCodecH265, true},
#endif
                                   {"VP8", Kind::kVp8, webrtc::kVideoCodecVP8, false},
                                   {"VP9", Kind::kVp9, webrtc::kVideoCodecVP9, false},
                                   {"AV1", Kind::kAv1, webrtc::kVideoCodecAV1, false},
                                   {"audio", Kind::kAudio, webrtc::kVideoCodecGeneric, false}};
#ifndef RTC_ENABLE_H265
  if (parent) std::printf("NOTE: this archive has no H.265 (RTC_ENABLE_H265 off): H.265 not covered\n");
#endif

  // The job list (identical in the parent and every child): job 0 = the guard control.
  std::vector<Job> jobs;
  jobs.push_back({"guard control", [] { return GuardControl(); }, true});
  for (const auto& cc : codecs) {
    if (ShortFrames(cc).empty()) {
      if (parent)
        std::printf("%-6s SHORT      none possible (clear header %s; empty frames are not sealed)\n", cc.name,
                    IsAudio(cc) ? "1 byte" : "0 bytes");
      continue;
    }
    jobs.push_back({std::string(cc.name) + " SHORT", [&cc, kp] { return RunShort(cc, kp); }});
  }
  for (const auto& cc : codecs)
    for (int ki : {0, 3})
      jobs.push_back({std::string(cc.name) + " BOUNDARY key index " + std::to_string(ki),
                      [&cc, kp, ki] { return RunRoundTrip(cc, kp, "BOUNDARY", BoundaryFrames(cc), ki); }});
  for (const auto& cc : codecs)
    jobs.push_back({std::string(cc.name) + " OVERSIZED", [&cc, kp] { return RunRoundTrip(cc, kp, "OVERSIZED", OversizedFrames(cc), 0); }});
  jobs.push_back({"H264 WIDE HEADER", [&codecs, kp] { return RunWideHeader(codecs[0], kp); }});
  for (const auto& v : vectors) {
    const CodecCase* cc = nullptr;
    for (const auto& c : codecs) {
      std::string n = c.name;
      for (auto& ch : n) ch = static_cast<char>(std::tolower(static_cast<unsigned char>(ch)));
      if (n == v.codec) cc = &c;
    }
    if (!cc) {
      if (v.codec == "h265") { if (parent) std::printf("vector %s skipped: no H.265 in this archive\n", v.hex.c_str()); continue; }
      std::printf("harness: vector codec %s unknown\n", v.codec.c_str());
      return 2;
    }
    jobs.push_back({std::string(cc->name) + " VECTOR " + v.hex.substr(0, 24), [cc, &v, kp] { return RunVector(*cc, v, kp); }});
  }

  if (!parent) {
    if (job >= static_cast<long>(jobs.size())) { std::printf("harness: no job %ld\n", job); return 2; }
    return RunHere(jobs[static_cast<size_t>(job)]);
  }
  // Every job in its own child, before this process starts any libwebrtc thread; the control first.
  for (size_t k = 0; k < jobs.size(); k++) {
    RunJob(k, jobs[k]);
    if (k == 0 && g_parent_harness) return 2;
  }

  if (g_parent_harness) {
    std::printf("HARNESS PROBLEM (%d)\n", g_parent_harness);
    return 2;
  }
  if (g_parent_fail) {
    std::printf("FAIL (%d):\n", g_parent_fail);
    for (const auto& f : g_parent_failed) std::printf("  - %s\n", f.c_str());
    return 1;
  }
  std::printf("PASS: no frame shorter than its clear header is sent, boundary/oversized frames seal and decrypt\n");
  return 0;
}
