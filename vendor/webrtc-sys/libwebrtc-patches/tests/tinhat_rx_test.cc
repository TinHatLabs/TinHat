// tinhat_rx_test.cc — the E2EE RECEIVER against IV layouts and malformed frames, through the REAL libwebrtc archive.
//
// Links a built libwebrtc (libwebrtc.a + its include/ tree) and drives its own FrameCryptorTransformer: the real
// decryptFrame, get_unencrypted_bytes, H264/H265::WriteRbsp + ParseRbsp, and for H.264 and AV1 the real RTP
// packetizer and depacketizer between sender and receiver. Frames come from two sources:
//   - hand-sealed (system OpenSSL AES-GCM under the transformer's own derived key, then the archive's WriteRbsp):
//     IV layouts upstream (ssrc|ts|ts-count), tinhat-iv-v1 (the 12.1.8 candidate: domain || 64-bit big-endian
//     counter, top bytes 00 00 00), tinhat-iv-v2 (zero-free), PLANTED IVs with `00 00 [00-03]` at every position
//     (an emulation byte inside the IV) and, for H.264/H.265, frames whose tag ends in 00 00 before an IV[0] <= 3
//     (the escape lands at the tag/IV junction);
//   - the archive's own SENDER (encryptFrame) for H.264, H.265, VP8, VP9, AV1 and audio.
// Positive sets: every frame comes back byte-identical, nothing else is delivered. Negative sets (another key, a
// flipped tag / ciphertext / IV / clear-header byte, short and malformed frames): NOTHING is delivered and the
// receiver survives (short frames run per codec in a child process with a timeout).
// The "upstream" set means upstream's IV allocation inside Tin Hat's framing (the AV1 frames carry Tin Hat's wrapper).
// A receiver that reads the IV from the raw tail before unescaping (upstream, the 12.1.9 libwebrtc) FAILS the v1 and
// planted H.264/H.265 sets; a hardened one (tinhat-iv-rx-v1) passes everything.
// Controls: the v1/planted sets must really carry an escape inside the 14-byte tail (counted).
//
// Build + run: run-tinhat-rx-test.sh <libwebrtc artifact dir> (the directory with lib/libwebrtc.a and include/).
// Exit 0 = every expectation held; 1 = a receiver failure (the list is printed); 2 = a harness/control problem.
#include <openssl/evp.h>
#include <signal.h>
#include <sys/wait.h>
#include <unistd.h>

#include <cerrno>
#include <cstdio>
#include <cstring>
#include <map>
#include <memory>
#include <mutex>
#include <optional>
#include <random>
#include <string>
#include <vector>

#include "api/crypto/frame_crypto_transformer.h"
#include "api/crypto/tinhat_iv.h"
#include "api/frame_transformer_interface.h"
#include "api/make_ref_counted.h"
#include "api/video/video_frame_metadata.h"
#include "common_video/h264/h264_common.h"
#include "modules/rtp_rtcp/source/rtp_format_h264.h"
#include "modules/rtp_rtcp/source/rtp_packet_to_send.h"
#include "modules/rtp_rtcp/source/rtp_packetizer_av1.h"
#include "modules/rtp_rtcp/source/rtp_video_header.h"
#include "modules/rtp_rtcp/source/video_rtp_depacketizer_av1.h"
#include "modules/rtp_rtcp/source/video_rtp_depacketizer_h264.h"
#include "rtc_base/copy_on_write_buffer.h"
#include "rtc_base/event.h"
#include "rtc_base/logging.h"
#ifdef RTC_ENABLE_H265
#include "common_video/h265/h265_common.h"
#endif

namespace webrtc {
// The names are TransformableFrameInterface::Passkey's friends (the upstream test mocks'), the only way to construct
// a frame outside libwebrtc. Not linked from the archive (its tests are not built), so no clash.
class MockTransformableVideoFrame : public TransformableVideoFrameInterface {
 public:
  MockTransformableVideoFrame(std::vector<uint8_t> data, VideoCodecType codec, bool key, uint32_t ssrc, uint32_t ts,
                              Direction dir)
      : TransformableVideoFrameInterface(Passkey()), data_(std::move(data)), key_(key), ssrc_(ssrc), ts_(ts),
        dir_(dir) {
    header_.codec = codec;
    header_.frame_type = key ? VideoFrameType::kVideoFrameKey : VideoFrameType::kVideoFrameDelta;
  }
  ArrayView<const uint8_t> GetData() const override { return data_; }
  void SetData(ArrayView<const uint8_t> d) override { data_.assign(d.begin(), d.end()); }
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
  std::vector<uint8_t> data_;
  bool key_;
  uint32_t ssrc_, ts_;
  Direction dir_;
  RTPVideoHeader header_;
};
class MockTransformableAudioFrame : public TransformableAudioFrameInterface {
 public:
  MockTransformableAudioFrame(std::vector<uint8_t> data, uint32_t ssrc, uint32_t ts, Direction dir)
      : TransformableAudioFrameInterface(Passkey()), data_(std::move(data)), ssrc_(ssrc), ts_(ts), dir_(dir) {}
  ArrayView<const uint8_t> GetData() const override { return data_; }
  void SetData(ArrayView<const uint8_t> d) override { data_.assign(d.begin(), d.end()); }
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
  std::vector<uint8_t> data_;
  uint32_t ssrc_, ts_;
  Direction dir_;
};
}  // namespace webrtc

namespace {
using webrtc::VideoCodecType;
using Bytes = std::vector<uint8_t>;
using Dir = webrtc::TransformableFrameInterface::Direction;

std::mt19937_64 rng(0x7a11ce5eedull);
uint8_t R8() { return static_cast<uint8_t>(rng()); }

enum class Kind { kH264, kH265, kVp8, kVp9, kAv1, kAudio };
struct CodecCase {
  const char* name;
  Kind kind;
  VideoCodecType codec;  // unused for audio
  bool nal;              // RBSP-escaped body (H.264/H.265)
};
bool IsAudio(const CodecCase& c) { return c.kind == Kind::kAudio; }

std::unique_ptr<webrtc::TransformableFrameInterface> MakeFrame(const CodecCase& c, Bytes data, bool key, uint32_t ssrc,
                                                                uint32_t ts, Dir dir) {
  if (IsAudio(c)) return std::make_unique<webrtc::MockTransformableAudioFrame>(std::move(data), ssrc, ts, dir);
  return std::make_unique<webrtc::MockTransformableVideoFrame>(std::move(data), c.codec, key, ssrc, ts, dir);
}

// Frames that reached a sink, by RTP timestamp (each frame of a run has its own).
class Sink : public webrtc::TransformedFrameCallback {
 public:
  void OnTransformedFrame(std::unique_ptr<webrtc::TransformableFrameInterface> f) override {
    std::lock_guard<std::mutex> l(mu_);
    auto d = f->GetData();
    if (out_.count(f->GetTimestamp())) dups_++;  // a frame delivered twice is a failure, never overwritten silently
    out_[f->GetTimestamp()] = Bytes(d.begin(), d.end());
    if (f->GetTimestamp() == sentinel_) ev_.Set();
  }
  void Expect(uint32_t sentinel) {
    std::lock_guard<std::mutex> l(mu_);
    sentinel_ = sentinel;
    ev_.Reset();
  }
  bool Wait() { return ev_.Wait(webrtc::TimeDelta::Seconds(20)); }
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
  return kp;
}

Cryptor MakeCryptor(webrtc::scoped_refptr<webrtc::KeyProvider> kp, bool audio) {
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
  c.t->SetKeyIndex(0);
  c.t->SetEnabled(true);
  return c;
}

bool Gcm(const Bytes& key, const Bytes& iv, const Bytes& aad, const Bytes& pt, Bytes* out) {
  const EVP_CIPHER* ci = key.size() == 16 ? EVP_aes_128_gcm() : key.size() == 32 ? EVP_aes_256_gcm() : nullptr;
  if (!ci) return false;
  EVP_CIPHER_CTX* x = EVP_CIPHER_CTX_new();
  int len = 0;
  out->assign(pt.size() + 16, 0);
  bool ok = EVP_EncryptInit_ex(x, ci, nullptr, nullptr, nullptr) == 1 &&
            EVP_CIPHER_CTX_ctrl(x, EVP_CTRL_GCM_SET_IVLEN, static_cast<int>(iv.size()), nullptr) == 1 &&
            EVP_EncryptInit_ex(x, nullptr, nullptr, key.data(), iv.data()) == 1 &&
            (aad.empty() || EVP_EncryptUpdate(x, nullptr, &len, aad.data(), static_cast<int>(aad.size())) == 1) &&
            (pt.empty() || EVP_EncryptUpdate(x, out->data(), &len, pt.data(), static_cast<int>(pt.size())) == 1) &&
            EVP_EncryptFinal_ex(x, out->data() + (pt.empty() ? 0 : len), &len) == 1 &&
            EVP_CIPHER_CTX_ctrl(x, EVP_CTRL_GCM_GET_TAG, 16, out->data() + pt.size()) == 1;
  EVP_CIPHER_CTX_free(x);
  return ok;
}

// --- plaintext frames (what an encoder hands the sender) ---
Bytes RandBody(size_t n) {
  Bytes b(n);
  for (auto& x : b) { x = R8(); if (x % 7 == 0) x = 0; }
  return b;
}
// H.264: [SPS + PPS +] IDR, or a non-IDR slice. Header rule (get_unencrypted_bytes): slice NAL offset + 2.
// Frames that take the RTP hop carry no SPS/PPS: the depacketizer parses (and may rewrite) a real SPS, these are
// placeholders. Their first slice byte 0xb8 parses as first_mb_in_slice 0, slice_type I, pps_id 0.
bool g_hop = false;
Bytes H264Frame(bool key, size_t n, size_t* header_len) {
  Bytes f;
  if (key && !g_hop) {
    f.insert(f.end(), {0, 0, 0, 1, 0x67, 0x42, 0xc0, 0x1f, 0x8c, 0x8d, 0x40});  // SPS (no zero runs)
    f.insert(f.end(), {0, 0, 0, 1, 0x68, 0xce, 0x3c, 0x80});                    // PPS
  }
  f.insert(f.end(), {0, 0, 0, 1, static_cast<uint8_t>(key ? 0x65 : 0x41)});
  *header_len = f.size() - 1 + 2;
  Bytes body = RandBody(n);
  body[0] = 0xb8;  // first slice byte (clear header)
  f.insert(f.end(), body.begin(), body.end());
  return f;
}
#ifdef RTC_ENABLE_H265
// H.265: [VPS SPS PPS +] IDR_W_RADL (19) / TRAIL_R (1); header = slice NAL offset + 2 (its 2-byte NAL header).
Bytes H265Frame(bool key, size_t n, size_t* header_len) {
  Bytes f;
  if (key) {
    f.insert(f.end(), {0, 0, 0, 1, 0x40, 0x01, 0x0c, 0x01, 0xff, 0xff});  // VPS
    f.insert(f.end(), {0, 0, 0, 1, 0x42, 0x01, 0x01, 0x01, 0x60});        // SPS
    f.insert(f.end(), {0, 0, 0, 1, 0x44, 0x01, 0xc1, 0x72});              // PPS
  }
  const uint8_t t = key ? 19 : 1;
  f.insert(f.end(), {0, 0, 0, 1, static_cast<uint8_t>(t << 1), 0x01});
  *header_len = f.size();
  Bytes body = RandBody(n);
  body[0] = 0xaf;
  f.insert(f.end(), body.begin(), body.end());
  return f;
}
#endif
Bytes MakePlain(const CodecCase& cc, bool key_frame, size_t n, size_t* hl) {
  switch (cc.kind) {
    case Kind::kH264: return H264Frame(key_frame, n, hl);
#ifdef RTC_ENABLE_H265
    case Kind::kH265: return H265Frame(key_frame, n, hl);
#endif
    default: break;
  }
  Bytes f = RandBody(n);
  if (cc.kind == Kind::kVp8) { *hl = key_frame ? 10 : 3; f[0] = key_frame ? 0x10 : 0x11; }
  else if (cc.kind == Kind::kVp9) { *hl = 0; f[0] = key_frame ? 0x82 : 0x86; }
  else if (cc.kind == Kind::kAv1) { *hl = 0; f[0] = 0x32; }
  else { *hl = 1; f[0] = 0x78; }  // audio: 1 clear byte (the Opus TOC)
  return f;
}

// --- IV layouts ---
using Iv = Bytes;
void PutBe32(Bytes* b, size_t at, uint32_t v) { for (int i = 0; i < 4; i++) (*b)[at + i] = static_cast<uint8_t>(v >> (24 - 8 * i)); }
struct V1Alloc {  // the 12.1.8 candidate: domain (32) || counter (64) BE, start (rand & 0xFF) << 32 | rand
  uint32_t domain = static_cast<uint32_t>(rng());
  uint64_t next = (static_cast<uint64_t>(R8()) << 32) | static_cast<uint32_t>(rng());
  Iv Take() {
    Iv iv(12);
    PutBe32(&iv, 0, domain);
    const uint64_t c = next++;
    PutBe32(&iv, 4, static_cast<uint32_t>(c >> 32));
    PutBe32(&iv, 8, static_cast<uint32_t>(c));
    return iv;
  }
};
Iv UpstreamIv(uint32_t ssrc, uint32_t ts, uint32_t count) {
  Iv iv(12);
  PutBe32(&iv, 0, ssrc);
  PutBe32(&iv, 4, ts);
  PutBe32(&iv, 8, ts - (count % 0xFFFF));
  return iv;
}
// An IV with `00 00 b` (b <= 3) planted at position p (0..9), or `00 00` at 10 (then T0 = 12 follows: no escape).
Iv PlantedIv(int p, uint8_t b) {
  Iv iv(12);
  for (auto& x : iv) x = static_cast<uint8_t>(1 + R8() % 255);
  iv[p] = 0;
  iv[p + 1] = 0;
  if (p + 2 < 12) iv[p + 2] = b;
  return iv;
}

enum class Corrupt { kNone, kTag, kCiphertext, kIv, kHeader };

// --- sealing the way a sender does (header clear + AAD; body sealed; tail escaped for H.264/H.265) ---
struct Sealed {
  Bytes wire;
  bool escaped_in_tail = false;  // the RAW 14-byte tail differs from IV || trailer
  Bytes tag_tail;                // the tag's last 2 bytes (junction search)
};
Sealed Seal(const CodecCase& cc, const Bytes& frame, size_t header_len, const Bytes& key, const Iv& iv, bool av1_key,
            Corrupt corrupt = Corrupt::kNone, uint8_t key_index = 0) {
  Bytes header(frame.begin(), frame.begin() + header_len), pt(frame.begin() + header_len, frame.end()), ct;
  if (!Gcm(key, iv, header, pt, &ct)) { std::fprintf(stderr, "harness: GCM seal failed\n"); std::exit(2); }
  Sealed s;
  s.tag_tail.assign(ct.end() - 2, ct.end());
  Bytes tail = ct;
  tail.insert(tail.end(), iv.begin(), iv.end());
  tail.push_back(static_cast<uint8_t>(iv.size()));
  tail.push_back(key_index);
  if (corrupt == Corrupt::kTag) tail[ct.size() - 1] ^= 0x01;
  if (corrupt == Corrupt::kCiphertext) tail[ct.size() > 16 ? (ct.size() - 16) / 2 : 0] ^= 0x01;
  if (corrupt == Corrupt::kIv) tail[ct.size() + 5] ^= 0x01;
  if (corrupt == Corrupt::kHeader) header.back() ^= 0x01;
  Bytes out = header;
  webrtc::Buffer esc;
  if (cc.kind == Kind::kH264) {
    webrtc::H264::WriteRbsp(tail, &esc);
#ifdef RTC_ENABLE_H265
  } else if (cc.kind == Kind::kH265) {
    webrtc::H265::WriteRbsp(tail, &esc);
#endif
  }
  if (cc.nal) {
    out.insert(out.end(), esc.data(), esc.data() + esc.size());
  } else if (cc.kind == Kind::kAv1) {
    if (av1_key) out.insert(out.end(), {0x0a, 0x00});
    out.push_back(0x30);
    out.insert(out.end(), tail.begin(), tail.end());
  } else {
    out.insert(out.end(), tail.begin(), tail.end());
  }
  s.escaped_in_tail = !std::equal(tail.end() - 14, tail.end(), out.end() - 14);
  s.wire = std::move(out);
  return s;
}

// --- RTP hops through the archive's packetizers and depacketizers ---
// H.264: Annex B reassembly as the receive path does it (start code before each NAL start; STAP-A split).
Bytes H264RtpHop(const Bytes& frame, int max_payload) {
  webrtc::RtpPacketizer::PayloadSizeLimits lim;
  lim.max_payload_len = max_payload;
  webrtc::RtpPacketizerH264 pk(frame, lim, webrtc::H264PacketizationMode::NonInterleaved);
  webrtc::VideoRtpDepacketizerH264 dp;
  Bytes out;
  webrtc::RtpPacketToSend pkt(nullptr);
  int packets = 0;
  while (pk.NextPacket(&pkt)) {
    packets++;
    auto parsed = dp.Parse(webrtc::CopyOnWriteBuffer(pkt.payload().data(), pkt.payload().size()));
    if (!parsed) { std::fprintf(stderr, "harness: H.264 depacketizer refused packet %d\n", packets); std::exit(2); }
    const auto& h = std::get<webrtc::RTPVideoHeaderH264>(parsed->video_header.video_type_header);
    const uint8_t* p = parsed->video_payload.cdata();
    const size_t n = parsed->video_payload.size();
    if (h.packetization_type == webrtc::kH264StapA) {
      size_t i = 1;
      while (i + 2 <= n) {
        const size_t len = (p[i] << 8) | p[i + 1];
        i += 2;
        if (i + len > n) { std::fprintf(stderr, "harness: bad STAP-A\n"); std::exit(2); }
        out.insert(out.end(), {0, 0, 0, 1});
        out.insert(out.end(), p + i, p + i + len);
        i += len;
      }
    } else {
      // a NAL starts at a single-NAL packet or an FU-A fragment with the S bit (raw RTP payload); the depacketizer's
      // is_first_packet_in_frame means "first slice of the picture", not "NAL start"
      const auto raw = pkt.payload();
      const bool fua = (raw[0] & 0x1f) == 28;
      if (h.packetization_type != webrtc::kH264FuA && fua) { std::fprintf(stderr, "harness: FU-A mismatch\n"); std::exit(2); }
      if (!fua || (raw[1] & 0x80)) out.insert(out.end(), {0, 0, 0, 1});
      out.insert(out.end(), p, p + n);
    }
  }
  if (packets < 2 && frame.size() > static_cast<size_t>(max_payload) + 4) {  // + the start code
    std::fprintf(stderr, "harness: no H.264 fragmentation (%d packets, frame %zu bytes)\n", packets, frame.size());
    std::exit(2);
  }
  return out;
}
// AV1: the packetizer's aggregation, then the depacketizer's AssembleFrame, which rewrites every OBU header with a
// size field (the wrapper arrives as 0x32 + LEB128, as on a real receiver).
Bytes Av1RtpHop(const Bytes& frame, bool key, int max_payload) {
  webrtc::RtpPacketizer::PayloadSizeLimits lim;
  lim.max_payload_len = max_payload;
  webrtc::RtpPacketizerAv1 pk(frame, lim, key ? webrtc::VideoFrameType::kVideoFrameKey : webrtc::VideoFrameType::kVideoFrameDelta, true);
  webrtc::VideoRtpDepacketizerAv1 dp;
  std::vector<Bytes> payloads;
  webrtc::RtpPacketToSend pkt(nullptr);
  while (pk.NextPacket(&pkt)) payloads.emplace_back(pkt.payload().begin(), pkt.payload().end());
  std::vector<webrtc::ArrayView<const uint8_t>> views;
  for (auto& p : payloads) views.emplace_back(p);
  auto img = dp.AssembleFrame(views);
  if (!img || payloads.empty()) { std::fprintf(stderr, "harness: AV1 depacketizer refused the frame\n"); std::exit(2); }
  return Bytes(img->data(), img->data() + img->size());
}

std::vector<std::string> g_fail;
int g_harness = 0;

// Feeds receiver frames (ts -> data, key) followed by a v2 sentinel; returns what was DELIVERED, by timestamp.
std::map<uint32_t, Bytes> Receive(Cryptor& rx, const CodecCase& cc, const std::map<uint32_t, std::pair<Bytes, bool>>& in,
                                  const Bytes& sentinel_frame) {
  rx.sink->Expect(kSentinelTs);
  for (const auto& [ts, fk] : in) rx.ft->Transform(MakeFrame(cc, fk.first, fk.second, kSsrc, ts, Dir::kReceiver));
  rx.ft->Transform(MakeFrame(cc, sentinel_frame, true, kSsrc, kSentinelTs, Dir::kReceiver));
  if (!rx.sink->Wait()) { std::fprintf(stderr, "harness: the sentinel frame never came back\n"); g_harness++; }
  auto got = rx.sink->Take();
  if (const int d = rx.sink->TakeDups()) {
    std::printf("  %d frames delivered more than once\n", d);
    g_fail.push_back(std::string(cc.name) + ": duplicate deliveries");
  }
  got.erase(kSentinelTs);
  return got;
}

Bytes SentinelFor(const CodecCase& cc, const Bytes& key) {
  size_t shl = 0;
  Bytes splain = MakePlain(cc, true, 200, &shl);
  tinhat::IvAllocator salloc(static_cast<uint32_t>(rng()), 7);
  auto siv = salloc.Next();
  return Seal(cc, splain, shl, key, Iv(siv->begin(), siv->end()), true).wire;
}

// Short and malformed frames for one codec, in this (child) process: header + 0..48 body bytes ending in a
// well-formed trailer (12, 0), a 2-byte body "0c 00", and trailers naming other IV lengths / key indices.
// Exit 0 = nothing delivered, 1 = something delivered, 2 = harness. A crash shows up as a signal in the parent.
int RunShort(const CodecCase& cc, webrtc::scoped_refptr<webrtc::KeyProvider> kp, const Bytes& key) {
  Cryptor rx = MakeCryptor(kp, IsAudio(cc));
  std::map<uint32_t, std::pair<Bytes, bool>> in;
  uint32_t ts = 1000;
  size_t hl = 0;
  Bytes plain = MakePlain(cc, true, 300, &hl);
  for (int kf = 0; kf < 2; kf++) {
    for (size_t body = 0; body <= 48; body++) {
      for (int variant = 0; variant < 3; variant++) {
        Bytes f(plain.begin(), plain.begin() + (cc.kind == Kind::kVp8 ? (kf ? 10 : 3) : hl));
        if (cc.kind == Kind::kAv1) f = {0x30};
        for (size_t i = 0; i < body; i++) f.push_back(static_cast<uint8_t>(0x40 + i));
        if (f.size() >= 2) {
          f[f.size() - 2] = variant == 1 ? static_cast<uint8_t>(body) : 12;  // variant 1: ivLength = the body size
          f[f.size() - 1] = variant == 2 ? 15 : 0;                           // variant 2: another (empty) key slot
        }
        if (f.empty()) continue;  // no clear header (VP9, audio) + no body = an EMPTY frame: not sent (see below)
        in[ts] = {f, kf != 0};
        ts += 3000;
      }
    }
  }
  // bodies of 30..48 bytes that pass the raw length check but shrink below tag || IV || trailer once unescaped
  // (H.264/H.265: every `00 00 03` loses its 03; other codecs: plain short bodies)
  for (size_t body = 30; body <= 48; body++) {
    Bytes f(plain.begin(), plain.begin() + (cc.kind == Kind::kVp8 ? 3 : hl));
    if (cc.kind == Kind::kAv1) f = {0x30};
    for (size_t i = 0; i + 2 < body; i += 3) f.insert(f.end(), {0, 0, 3});
    f.insert(f.end(), {12, 0});
    in[ts] = {f, false};
    ts += 3000;
  }
  // (an EMPTY frame is not sent: upstream forwards it unless discard_frame_when_cryptor_not_ready, an option the
  // KeyProviderOptions copy constructor leaves uninitialized; that policy is not part of this change)
  auto got = Receive(rx, cc, in, SentinelFor(cc, key));
  std::printf("%-26s sent %4zu  short/malformed frames                       delivered %4zu\n",
              (std::string(cc.name) + " short/malformed").c_str(), in.size(), got.size());
  std::fflush(stdout);
  return !got.empty() ? 1 : (g_harness ? 2 : 0);
}

// Runs RunShort in a child with a timeout; the parent records a crash / hang / delivery as a receiver failure.
void ShortInChild(const CodecCase& cc, webrtc::scoped_refptr<webrtc::KeyProvider> kp, const Bytes& key) {
  std::fflush(stdout);
  const pid_t pid = fork();
  if (pid < 0) { std::printf("harness: fork failed (%s)\n", std::strerror(errno)); g_harness++; return; }
  if (pid == 0) {
    alarm(60);  // a hang is a failure, not a stuck test
    const int r = RunShort(cc, kp, key);
    std::fflush(stdout);
    _exit(r);
  }
  int st = 0;
  pid_t w;
  do { w = waitpid(pid, &st, 0); } while (w < 0 && errno == EINTR);
  const std::string label = std::string(cc.name) + " short/malformed";
  if (w != pid) { std::printf("harness: waitpid failed\n"); g_harness++; return; }
  if (WIFSIGNALED(st)) {
    std::printf("%-26s the receiver DIED (signal %d%s)\n", label.c_str(), WTERMSIG(st),
                WTERMSIG(st) == SIGALRM ? ", hung" : "");
    g_fail.push_back(label + ": receiver died (signal " + std::to_string(WTERMSIG(st)) + ")");
  } else if (!WIFEXITED(st) || WEXITSTATUS(st) == 1) {
    g_fail.push_back(label + ": a malformed frame was delivered");
  } else if (WEXITSTATUS(st) != 0) {
    g_harness++;
  }
}

}  // namespace

int main(int argc, char** argv) {
  const bool verbose = argc > 1 && std::string(argv[1]) == "-v";
  webrtc::LogMessage::LogToDebug(verbose ? webrtc::LS_INFO : webrtc::LS_NONE);
  const int frames_per_set = 300;

  Bytes material(32), other(32);
  for (auto& x : material) x = R8();
  for (auto& x : other) x = R8();
  auto kp = MakeKeyProvider(material);
  auto kp_other = MakeKeyProvider(other);
  const Bytes key = kp->GetSharedKey(kPid)->GetKeySet(0)->encryption_key;
  const Bytes key_other = kp_other->GetSharedKey(kPid)->GetKeySet(0)->encryption_key;
  if (key.empty() || key == key_other) { std::fprintf(stderr, "harness: no derived key\n"); return 2; }

  std::vector<CodecCase> codecs = {{"H264", Kind::kH264, webrtc::kVideoCodecH264, true},
#ifdef RTC_ENABLE_H265
                                   {"H265", Kind::kH265, webrtc::kVideoCodecH265, true},
#endif
                                   {"VP8", Kind::kVp8, webrtc::kVideoCodecVP8, false},
                                   {"VP9", Kind::kVp9, webrtc::kVideoCodecVP9, false},
                                   {"AV1", Kind::kAv1, webrtc::kVideoCodecAV1, false},
                                   {"audio", Kind::kAudio, webrtc::kVideoCodecGeneric, false}};
#ifndef RTC_ENABLE_H265
  std::printf("NOTE: this archive has no H.265 (RTC_ENABLE_H265 off): H.265 not covered\n");
#endif

  // Short/malformed frames first, one child per codec, before this process starts any libwebrtc thread.
  for (const auto& cc : codecs) ShortInChild(cc, kp, key);

  uint32_t ts_base = 1000;
  for (const auto& cc : codecs) {
    Cryptor rx = MakeCryptor(kp, IsAudio(cc));
    Cryptor tx = MakeCryptor(kp, IsAudio(cc));
    const Bytes sentinel = SentinelFor(cc, key);
    const bool has_hop = cc.kind == Kind::kH264 || cc.kind == Kind::kAv1;

    const std::vector<std::string> sets = {"upstream", "v1", "v2", "planted", "junction", "keyindex",
                                           "otherkey", "bad-tag", "bad-ciphertext", "bad-iv", "bad-header"};
    for (const auto& set : sets) {
      if (set == "junction" && !cc.nal) continue;
      for (int hop = 0; hop < (has_hop ? 2 : 1); hop++) {
        g_hop = hop != 0;
        V1Alloc v1;
        tinhat::IvAllocator v2(static_cast<uint32_t>(rng()), rng() % 1000000);
        const bool negative = set.rfind("bad-", 0) == 0 || set == "otherkey";
        std::map<uint32_t, std::pair<Bytes, bool>> in;
        std::map<uint32_t, Bytes> want;
        int escaped = 0, skipped = 0;
        const int n = set == "planted" ? 11 * 4 * 6 : set == "junction" ? 16 : frames_per_set;
        for (int i = 0; i < n; i++) {
          const bool kf = i % 30 == 0;
          const uint32_t ts = ts_base + static_cast<uint32_t>(i) * 3000;
          size_t hl = 0;
          Bytes plain;
          Sealed s;
          if (set == "junction") {
            // reseal until the tag ends in 00 00, IV[0] in 0..3: the escape lands exactly at the tag/IV junction
            Iv iv = PlantedIv(5, 1);
            iv[0] = static_cast<uint8_t>(i % 4);
            for (int tries = 0;; tries++) {
              plain = MakePlain(cc, kf, 40 + rng() % 200, &hl);
              s = Seal(cc, plain, hl, key, iv, kf);
              if (s.tag_tail[0] == 0 && s.tag_tail[1] == 0) break;
              if (tries > 3000000) { std::fprintf(stderr, "harness: no junction frame found\n"); return 2; }
            }
          } else {
            plain = MakePlain(cc, kf, 60 + rng() % 2400, &hl);
            if (negative && set == "bad-header" && hl == 0) { skipped++; continue; }  // no clear header to flip
            Iv iv;
            if (set == "upstream") iv = UpstreamIv(kSsrc, ts, static_cast<uint32_t>(i));
            else if (set == "v1") iv = v1.Take();
            else if (set == "planted") iv = PlantedIv((i / 6) % 11, static_cast<uint8_t>((i / 66) % 4));
            else { auto x = v2.Next(); iv.assign(x->begin(), x->end()); }
            const Corrupt c = set == "bad-tag" ? Corrupt::kTag : set == "bad-ciphertext" ? Corrupt::kCiphertext
                              : set == "bad-iv" ? Corrupt::kIv : set == "bad-header" ? Corrupt::kHeader : Corrupt::kNone;
            // keyindex: the trailer names slot 3, which holds the same key (shared key set into slot 3 below)
            s = Seal(cc, plain, hl, set == "otherkey" ? key_other : key, iv, kf, c, set == "keyindex" ? 3 : 0);
          }
          if (hop && cc.kind == Kind::kH264) s.wire = H264RtpHop(s.wire, 400);
          if (hop && cc.kind == Kind::kAv1) s.wire = Av1RtpHop(s.wire, kf, 400);
          in[ts] = {s.wire, kf};
          want[ts] = plain;
          if (s.escaped_in_tail) escaped++;
        }
        if (set == "keyindex") kp->SetSharedKey(3, material);
        auto got = Receive(rx, cc, in, sentinel);
        int back = 0, wrong = 0;
        for (const auto& [ts, w] : want) {
          auto it = got.find(ts);
          if (it == got.end()) continue;
          if (it->second == w) back++; else wrong++;
        }
        const size_t extra = got.size() - static_cast<size_t>(back + wrong);
        const std::string label = std::string(cc.name) + " " + set + (hop ? " (RTP hop)" : "");
        std::printf("%-30s sent %4zu  escape inside the raw IV/trailer tail %4d  %s %4d%s\n", label.c_str(), in.size(),
                    escaped, negative ? "delivered" : "decrypted", negative ? static_cast<int>(got.size()) : back,
                    wrong ? "  (WRONG BYTES DELIVERED)" : "");
        if (extra) { std::printf("  harness: %zu deliveries for frames never sent\n", extra); g_harness++; }
        if (negative) {
          if (!got.empty()) g_fail.push_back(label + ": " + std::to_string(got.size()) + " frames delivered (must be 0)");
        } else if (back != static_cast<int>(in.size()) || wrong) {
          g_fail.push_back(label + ": " + std::to_string(in.size() - back) + "/" + std::to_string(in.size()) +
                           " not decrypted, " + std::to_string(wrong) + " delivered with wrong bytes");
        }
        // the sets that exist to exercise the bug must really carry escapes inside the tail (NAL codecs only)
        if (cc.nal && (set == "v1" || set == "planted") && escaped < static_cast<int>(in.size()) / 2) {
          std::printf("  CONTROL BROKEN: the %s set carries too few in-tail escapes\n", set.c_str());
          g_harness++;
        }
        if (cc.nal && set == "v2" && escaped != 0) g_fail.push_back(label + ": v2 IVs put an escape in the tail");
        (void)skipped;
        ts_base += 4000000;
      }
    }

    // The archive's own SENDER -> (H.264 / AV1: RTP hop) -> receiver.
    for (int hop = 0; hop < (has_hop ? 2 : 1); hop++) {
      g_hop = hop != 0;
      std::map<uint32_t, std::pair<Bytes, bool>> rin;
      std::map<uint32_t, Bytes> want;
      const int n = frames_per_set;
      const uint32_t last = ts_base + static_cast<uint32_t>(n - 1) * 3000;
      tx.sink->Expect(last);
      std::map<uint32_t, bool> kfs;
      for (int i = 0; i < n; i++) {
        const bool kf = i % 30 == 0;
        size_t hl = 0;
        Bytes plain = MakePlain(cc, kf, 60 + rng() % 2400, &hl);
        const uint32_t ts = ts_base + static_cast<uint32_t>(i) * 3000;
        want[ts] = plain;
        kfs[ts] = kf;
        tx.ft->Transform(MakeFrame(cc, plain, kf, kSsrc, ts, Dir::kSender));
      }
      const bool sender_done = tx.sink->Wait();
      auto sealed = tx.sink->Take();
      int dropped = 0;
      for (auto& [ts, plain] : want) {
        if (!sealed.count(ts)) { dropped++; continue; }
        Bytes w = sealed[ts];
        if (hop && cc.kind == Kind::kH264) w = H264RtpHop(w, 400);
        if (hop && cc.kind == Kind::kAv1) w = Av1RtpHop(w, kfs[ts], 400);
        rin[ts] = {w, kfs[ts]};
      }
      // only the VP9 keyframe IV step may drop a frame (a cap miss, p = (15/16)^512 per keyframe)
      const int allowed_drops = cc.kind == Kind::kVp9 ? 2 : 0;
      if (!sender_done && allowed_drops == 0) { std::printf("  sender: last frame missing\n"); }
      auto got = Receive(rx, cc, rin, sentinel);
      int back = 0;
      for (auto& [ts, fk] : rin) if (got.count(ts) && got[ts] == want[ts]) back++;
      const std::string label = std::string(cc.name) + " archive sender" + (hop ? " (RTP hop)" : "");
      std::printf("%-30s sent %4zu  (sender dropped %d)                       decrypted %4d\n", label.c_str(),
                  rin.size(), dropped, back);
      if (back != static_cast<int>(rin.size()) || dropped > allowed_drops || got.size() != rin.size())
        g_fail.push_back(label + ": " + std::to_string(rin.size() - back) + " lost, " + std::to_string(dropped) + " dropped");
      ts_base += 4000000;
    }
    g_hop = false;
    rx.t->SetEnabled(false);
    tx.t->SetEnabled(false);
  }

  std::printf("\n");
  if (g_harness) { std::printf("HARNESS/CONTROL PROBLEMS: %d\n", g_harness); return 2; }
  if (!g_fail.empty()) {
    std::printf("RECEIVER FAIL (%zu):\n", g_fail.size());
    for (auto& f : g_fail) std::printf("  %s\n", f.c_str());
    return 1;
  }
  std::printf("PASS: every IV layout, every codec + audio, malformed frames refused, through the archive's own receiver\n");
  return 0;
}
