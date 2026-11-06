// tinhat_iv_test.cc — the IV-uniqueness, H.264-safety and VP9 keyframe-IV test for the patched libwebrtc (tinhat-iv-v2 +
// tinhat-vp9-kf-iv-v1). Compiles api/crypto/tinhat_iv.h AS SHIPPED (extracted from vp9-e2ee-keyframe-iv.patch by
// run-tinhat-iv-test.sh) against OpenSSL's AES-128-GCM. Mirror: Tin Hat Web tools/iv-unique-unit.mjs (the worker's
// real encodeFunction). Exit 0 = every check passed.
#include <openssl/evp.h>
#include <openssl/rand.h>

#include <cstdio>
#include <cstring>
#include <algorithm>
#include <map>
#include <set>
#include <string>
#include <unordered_set>
#include <random>
#include <vector>

#include "api/crypto/tinhat_iv.h"

namespace {
int fails = 0;
void check(bool ok, const std::string& what) {
  std::printf("%s  %s\n", ok ? "PASS" : "FAIL", what.c_str());
  if (!ok) fails++;
}
// AES-128-GCM seal (12-byte IV, empty AAD): ciphertext || tag.
std::vector<uint8_t> seal(const uint8_t key[16], const tinhat::Iv& iv, const uint8_t* p, size_t n) {
  EVP_CIPHER_CTX* c = EVP_CIPHER_CTX_new();
  std::vector<uint8_t> out(n + 16);
  int l = 0, l2 = 0;
  EVP_EncryptInit_ex(c, EVP_aes_128_gcm(), nullptr, nullptr, nullptr);
  EVP_CIPHER_CTX_ctrl(c, EVP_CTRL_GCM_SET_IVLEN, 12, nullptr);
  EVP_EncryptInit_ex(c, nullptr, nullptr, key, iv.data());
  if (n) EVP_EncryptUpdate(c, out.data(), &l, p, static_cast<int>(n));
  EVP_EncryptFinal_ex(c, out.data() + l, &l2);
  EVP_CIPHER_CTX_ctrl(c, EVP_CTRL_GCM_GET_TAG, 16, out.data() + n);
  EVP_CIPHER_CTX_free(c);
  return out;
}
// the counter back from the v2 layout: 8 base-255 digits, each + 1, most significant first
uint64_t ctr_of(const tinhat::Iv& iv) {
  uint64_t c = 0;
  for (int i = 4; i < 12; i++) c = c * 255 + static_cast<uint64_t>(iv[i] - 1);
  return c;
}
// libwebrtc's H264::WriteRbsp / ParseRbsp / NeedsRbspUnescaping (common_video/h264/h264_common.cc; H.265 is the same)
std::vector<uint8_t> write_rbsp(const std::vector<uint8_t>& in) {
  std::vector<uint8_t> out;
  int zeros = 0;
  for (uint8_t b : in) {
    if (zeros >= 2 && b <= 3) { out.push_back(3); zeros = 0; }
    out.push_back(b);
    zeros = b == 0 ? zeros + 1 : 0;
  }
  return out;
}
bool needs_unescape(const std::vector<uint8_t>& d) {
  for (size_t i = 0; i + 2 < d.size(); i++) if (d[i] == 0 && d[i + 1] == 0 && d[i + 2] == 3) return true;
  return false;
}
std::vector<uint8_t> parse_rbsp(const std::vector<uint8_t>& d) {
  std::vector<uint8_t> out;
  for (size_t i = 0; i < d.size();) {
    if (d.size() - i >= 3 && d[i] == 0 && d[i + 1] == 0 && d[i + 2] == 3) { out.push_back(0); out.push_back(0); i += 3; }
    else out.push_back(d[i++]);
  }
  return out;
}
// a 00 00 [00-03] run anywhere in d (what makes WriteRbsp insert an escape)
bool has_escape_run(const std::vector<uint8_t>& d) {
  for (size_t i = 0; i + 2 < d.size(); i++) if (d[i] == 0 && d[i + 1] == 0 && d[i + 2] <= 3) return true;
  return false;
}
// AES-128-GCM open with AAD; true = authentic
bool open_gcm(const uint8_t key[16], const uint8_t* iv, const std::vector<uint8_t>& aad, const std::vector<uint8_t>& ct_tag,
              std::vector<uint8_t>* plain) {
  if (ct_tag.size() < 16) return false;
  const size_t n = ct_tag.size() - 16;
  EVP_CIPHER_CTX* c = EVP_CIPHER_CTX_new();
  plain->assign(n, 0);
  int l = 0, l2 = 0;
  EVP_DecryptInit_ex(c, EVP_aes_128_gcm(), nullptr, nullptr, nullptr);
  EVP_CIPHER_CTX_ctrl(c, EVP_CTRL_GCM_SET_IVLEN, 12, nullptr);
  EVP_DecryptInit_ex(c, nullptr, nullptr, key, iv);
  if (!aad.empty()) EVP_DecryptUpdate(c, nullptr, &l, aad.data(), static_cast<int>(aad.size()));
  if (n) EVP_DecryptUpdate(c, plain->data(), &l, ct_tag.data(), static_cast<int>(n));
  EVP_CIPHER_CTX_ctrl(c, EVP_CTRL_GCM_SET_TAG, 16, const_cast<uint8_t*>(ct_tag.data() + n));
  const int ok = EVP_DecryptFinal_ex(c, plain->data() + l, &l2);
  EVP_CIPHER_CTX_free(c);
  return ok > 0;
}
// AES-128-GCM seal with AAD: ciphertext || tag
std::vector<uint8_t> seal_aad(const uint8_t key[16], const uint8_t* iv, const std::vector<uint8_t>& aad, const std::vector<uint8_t>& p) {
  EVP_CIPHER_CTX* c = EVP_CIPHER_CTX_new();
  std::vector<uint8_t> out(p.size() + 16);
  int l = 0, l2 = 0;
  EVP_EncryptInit_ex(c, EVP_aes_128_gcm(), nullptr, nullptr, nullptr);
  EVP_CIPHER_CTX_ctrl(c, EVP_CTRL_GCM_SET_IVLEN, 12, nullptr);
  EVP_EncryptInit_ex(c, nullptr, nullptr, key, iv);
  if (!aad.empty()) EVP_EncryptUpdate(c, nullptr, &l, aad.data(), static_cast<int>(aad.size()));
  if (!p.empty()) EVP_EncryptUpdate(c, out.data(), &l, p.data(), static_cast<int>(p.size()));
  EVP_EncryptFinal_ex(c, out.data() + l, &l2);
  EVP_CIPHER_CTX_ctrl(c, EVP_CTRL_GCM_GET_TAG, 16, out.data() + p.size());
  EVP_CIPHER_CTX_free(c);
  return out;
}
// One E2EE H.264 frame the way the patched libwebrtc sends it and EVERY receiver in users' hands reads it (the
// upstream decryptFrame: trailer + IV from the RAW tail, then ParseRbsp over the rest, then GCM). Returns true when
// the receiver gets the plaintext back.
// hardened = the tinhat-iv-rx-v1 receiver (unescape the whole body FIRST, then tinhat::SplitRxTail as shipped).
bool h264_round_trip(const uint8_t key[16], const tinhat::Iv& iv, std::mt19937& rng, bool hardened = false) {
  std::vector<uint8_t> frame = {0, 0, 0, 1, 0x65};  // Annex B start code + IDR NAL header
  const size_t n = 40 + rng() % 400;
  for (size_t i = 0; i < n; i++) frame.push_back(rng() % 7 == 0 ? 0 : static_cast<uint8_t>(rng()));
  const size_t clear = 4 + 2;  // get_unencrypted_bytes: payload_start_offset + 2 for an IDR/slice NAL
  std::vector<uint8_t> aad(frame.begin(), frame.begin() + clear), payload(frame.begin() + clear, frame.end());
  std::vector<uint8_t> body = seal_aad(key, iv.data(), aad, payload);
  body.insert(body.end(), iv.begin(), iv.end());
  body.push_back(12);
  body.push_back(static_cast<uint8_t>(rng() % 255));
  std::vector<uint8_t> wire = aad;
  const std::vector<uint8_t> esc = write_rbsp(body);
  wire.insert(wire.end(), esc.begin(), esc.end());
  if (hardened) {
    std::vector<uint8_t> enc(wire.begin() + clear, wire.end());
    if (needs_unescape(enc)) enc = parse_rbsp(enc);
    tinhat::RxTail t;
    if (!tinhat::SplitRxTail(enc.data(), enc.size(), 12, tinhat::kGcmTagLen, &t)) return false;
    std::vector<uint8_t> ct(enc.begin(), enc.begin() + t.ct_len), back;
    return open_gcm(key, enc.data() + t.iv_off, aad, ct, &back) && back == payload;
  }
  // the receiver (upstream decryptFrame)
  const uint8_t iv_len = wire[wire.size() - 2];
  if (iv_len != 12) return false;
  const uint8_t* rx_iv = wire.data() + wire.size() - 2 - iv_len;
  std::vector<uint8_t> enc(wire.begin() + clear, wire.end());
  if (needs_unescape(enc)) enc = parse_rbsp(enc);
  std::vector<uint8_t> ct(enc.begin(), enc.end() - iv_len - 2), back;
  return open_gcm(key, rx_iv, aad, ct, &back) && back == payload;
}
std::string hex(const tinhat::Iv& iv) {
  static const char* d = "0123456789abcdef";
  std::string s;
  for (uint8_t b : iv) { s += d[b >> 4]; s += d[b & 15]; }
  return s;
}
}  // namespace

int main() {
  // 1. Predicate = the shared vectors' accept set (16 bytes; vectors.json is checked byte-for-byte by the Rust test).
  int accept = 0;
  for (int b = 0; b < 256; b++) accept += tinhat::Vp9ByteLooksLikeKeyFrame(static_cast<uint8_t>(b));
  check(accept == 16, "predicate accepts 16 of 256 bytes");
  check(tinhat::Vp9ByteLooksLikeKeyFrame(0x82) && !tinhat::Vp9ByteLooksLikeKeyFrame(0x86) &&
            tinhat::Vp9ByteLooksLikeKeyFrame(0xb0) && !tinhat::Vp9ByteLooksLikeKeyFrame(0xb4),
        "predicate cases 0x82 key, 0x86 delta, 0xb0 profile-3 key, 0xb4 profile-3 delta");

  // 2. The long session, every IV recorded. ONE allocator = one process. Its start is FORCED just below the 32-bit
  //    carry so the low counter word wraps mid-run. Tracks: mic (audio, 1 clear byte, ssrc A), camera (VP8-like, ssrc
  //    B), the share (VP9, 3 SVC spatial layers per picture = 3 frames with the SAME timestamp, ssrc C; a keyframe
  //    every 30 pictures), all three with a CONSTANT timestamp for stretches of 500 frames (equal timestamps), a key
  //    rotation every 20000 frames (a fresh random key), a ratchet every 7000 (a derived key = another key), a
  //    reconnect at the middle (new cryptors, new SSRCs, same key, SAME process allocator).
  const uint64_t kStart = 4228250625ull - 16384;  // 16384 below a 4-digit base-255 carry (255^4)
  tinhat::IvAllocator alloc(0x5a5a5a5au, kStart);
  std::map<std::string, std::unordered_set<std::string>> seen;  // the ACTUAL key bytes -> IVs sent under them
  int key_gen = 0, dups = 0, vp9_frames = 0, vp9_ok = 0, matched_full = 0, full_checked = 0, drops = 0;
  uint8_t key[16];
  RAND_bytes(key, 16);
  long ivs = 0;
  uint64_t max_ctr = 0;
  auto take = [&](const tinhat::Iv& iv) {
    ivs++;
    if (!seen[std::string(reinterpret_cast<const char*>(key), 16)].insert(hex(iv)).second) dups++;
    max_ctr = std::max(max_ctr, ctr_of(iv));
  };
  const int kPictures = 60000;
  for (int pic = 0; pic < kPictures; pic++) {
    if (pic > 0 && pic % 20000 == 0) { RAND_bytes(key, 16); key_gen++; }          // rotation
    else if (pic > 0 && pic % 7000 == 0) { key[0] ^= 0x5c; key_gen++; }           // "ratchet": every 2nd one RETURNS to an earlier key (indexed by key bytes, so a returning key must still never repeat an IV)
    // (pic == kPictures/2: reconnect — new cryptors and SSRCs, same key, same allocator: nothing to do here,
    //  because neither SSRC nor timestamp is part of the IV any more)
    // mic + camera: one IV each from makeIv
    for (int t = 0; t < 2; t++) { auto iv = alloc.Next(); if (!iv) { drops++; continue; } take(*iv); }
    // share: 3 SVC layer frames, same timestamp; VP9 keyframe IV step with a REAL one-byte AES-GCM probe
    const bool key_pic = pic % 30 == 0;
    for (int layer = 0; layer < 3; layer++) {
      uint8_t frame[64];
      RAND_bytes(frame, sizeof frame);
      frame[0] = key_pic && layer == 0 ? 0x82 : 0x86;
      const bool want_key = key_pic && layer == 0;
      auto r = tinhat::StepVp9Iv(alloc, want_key, [&](const tinhat::Iv& cand, uint8_t* c0) {
        auto ct = seal(key, cand, frame, 1);  // the private probe: one byte
        *c0 = ct[0];
        return true;
      });
      vp9_frames++;
      if (r.step != tinhat::Vp9IvStep::kMatched) { drops++; continue; }
      take(r.iv);  // only the chosen IV is ever sent; the rejected candidates are consumed (never handed out again)
      // every 50th frame: the REAL full seal's byte 0 = the probe's, and gives the SFU the right verdict
      if (vp9_frames % 50 == 0) {
        full_checked++;
        auto full = seal(key, r.iv, frame, sizeof frame);
        auto one = seal(key, r.iv, frame, 1);
        if (full[0] == one[0] && tinhat::Vp9ByteLooksLikeKeyFrame(full[0]) == want_key) matched_full++;
      }
      vp9_ok++;
    }
  }
  check(dups == 0, "no (key, IV) pair repeats: " + std::to_string(ivs) + " IVs sent under " + std::to_string(seen.size()) + " distinct keys (" + std::to_string(key_gen + 1) + " key changes), " + std::to_string(dups) + " repeats");
  check(max_ctr >= 4228250625ull && kStart < 4228250625ull, "the counter crossed a 4-digit base-255 carry mid-run (255^4; start " + std::to_string(kStart) + ", end " + std::to_string(max_ctr) + ")");
  check(drops == 0 && vp9_ok == vp9_frames, "every VP9 frame got a matching IV within the cap (" + std::to_string(vp9_ok) + "/" + std::to_string(vp9_frames) + ")");
  check(full_checked > 100 && matched_full == full_checked, "full-seal byte 0 = probe byte 0 and the SFU verdict = frame type (" + std::to_string(matched_full) + "/" + std::to_string(full_checked) + ")");

  // 3. Controls (the detector works, and what the argument depends on):
  //    (a) the SAME key in TWO counter domains that happen to share domain and start: repeats are caught. This is
  //        why every key must be sealed in exactly one domain (README "IV uniqueness", premise P1).
  {
    tinhat::IvAllocator a1(7, 1000), a2(7, 1000);
    std::unordered_set<std::string> s;
    int d = 0;
    for (int i = 0; i < 5000; i++) {
      if (!s.insert(hex(*a1.Next())).second) d++;
      if (!s.insert(hex(*a2.Next())).second) d++;
    }
    check(d == 5000, "control (a): one key in two domains with equal domain+start is caught (" + std::to_string(d) + " repeats)");
  }
  //    (b) upstream's rule (ssrc | ts | ts - count % 0xFFFF, per-cryptor counter, native start 0) in its worst case:
  //        a reconnect under the same key whose new sender reuses the SSRC and the RTP timestamp base.
  {
    std::unordered_set<std::string> s;
    int d = 0;
    for (int session = 0; session < 2; session++) {
      uint32_t c = 0;  // the native counter's effective start (its random init is never used)
      for (int pic = 0; pic < 3000; pic++) {
        const uint32_t ssrc = 0x1111u, ts = 90000u + static_cast<uint32_t>(pic) * 3000u;
        char b[64];
        std::snprintf(b, sizeof b, "%08x%08x%08x", ssrc, ts, ts - (c % 0xFFFF));
        c++;
        if (!s.insert(b).second) d++;
      }
    }
    check(d == 3000, "control (b): upstream's rule repeats every IV after a same-key reconnect that reuses SSRC + timestamp base (" + std::to_string(d) + " repeats)");
  }

  // 4. Exhaustion: an allocator at the end of its counter refuses (never wraps to a used value).
  {
    tinhat::IvAllocator a(1, tinhat::IvAllocator::kCounterLimit - 2);
    auto x = a.Next(), y = a.Next(), z = a.Next();
    check(x && y && !z, "exhausted allocator refuses at 255^8 (no wrap)");
    auto r = tinhat::StepVp9Iv(a, true, [](const tinhat::Iv&, uint8_t* c0) { *c0 = 0x82; return true; });
    check(r.step == tinhat::Vp9IvStep::kExhausted, "VP9 step on an exhausted allocator = kExhausted (frame dropped)");
  }
  // 5. Cap miss and probe failure are reported (the caller drops the frame); every candidate is consumed.
  {
    tinhat::IvAllocator a(2, 0);
    auto r = tinhat::StepVp9Iv(a, true, [](const tinhat::Iv&, uint8_t* c0) { *c0 = 0x00; return true; });
    check(r.step == tinhat::Vp9IvStep::kCapMiss && r.tries == tinhat::kVp9KfIvMaxTries, "never-matching probe = kCapMiss after 512 tries");
    auto n = a.Next();
    check(n && ctr_of(*n) == static_cast<uint64_t>(tinhat::kVp9KfIvMaxTries), "the 512 rejected candidates were consumed (next counter = 512)");
    auto f = tinhat::StepVp9Iv(a, false, [](const tinhat::Iv&, uint8_t*) { return false; });
    check(f.step == tinhat::Vp9IvStep::kSealFailed && f.tries == 1, "a probe that fails to seal = kSealFailed (frame dropped)");
    int cap_hits = 0, keys = 0;
    for (int i = 0; i < 4000; i++) {
      uint8_t k2[16]; RAND_bytes(k2, 16); uint8_t first = 0x82;
      auto q = tinhat::StepVp9Iv(a, true, [&](const tinhat::Iv& cand, uint8_t* c0) { *c0 = seal(k2, cand, &first, 1)[0]; return true; }, 1);
      keys++;
      if (q.step == tinhat::Vp9IvStep::kCapMiss) cap_hits++;
    }
    check(cap_hits > keys * 85 / 100 && cap_hits < keys * 97 / 100, "with the cap forced to 1, ~15/16 of keyframes take the drop path (" + std::to_string(cap_hits) + "/" + std::to_string(keys) + ")");
  }
  // 6. What byte 0 shows: over random plaintext first bytes, a matched keyframe's byte 0 is spread over the 16
  //    "keyframe" values and a delta frame's over the 240 others, whatever plaintext byte 0 was.
  {
    tinhat::IvAllocator a(3, 0);
    std::map<int, std::set<int>> by_plain;  // plaintext byte 0 -> ciphertext byte 0 values seen (keyframes)
    int indep = 0;
    for (int p0 : {0x82, 0x00, 0xff}) {
      std::map<int, int> hist;
      for (int i = 0; i < 3200; i++) {
        uint8_t k2[16]; RAND_bytes(k2, 16); uint8_t first = static_cast<uint8_t>(p0);
        auto q = tinhat::StepVp9Iv(a, true, [&](const tinhat::Iv& cand, uint8_t* c0) { *c0 = seal(k2, cand, &first, 1)[0]; return true; });
        hist[seal(k2, q.iv, &first, 1)[0]]++;
      }
      bool flat = hist.size() == 16;
      for (auto& [b, n] : hist) flat = flat && tinhat::Vp9ByteLooksLikeKeyFrame(b) && n > 120 && n < 290;
      if (flat) indep++;
    }
    check(indep == 3, "keyframe byte 0 is uniform over the 16 keyframe values for plaintext byte 0 = 0x82, 0x00, 0xff");
  }
  // 7. H.264-safe layout (tinhat-iv-v2): over a long run, every IV byte is nonzero and byte 0 >= 4, so neither the IV,
  //    nor IV || trailer (key indexes 0/1/3/254), nor the tag/IV junction (representative tag tails 00 00, 00 01, 01 00,
  //    00 03, ff 00; the general guarantee is the byte argument in tinhat_iv.h) contains a
  //    00 00 [00-03] run whose escape would land inside the 14-byte tail.
  {
    tinhat::IvAllocator a(0x00000000u, 255ull * 255 - 4000);  // an all-zero random domain, across a 2-digit carry
    int zero_bytes = 0, low_b0 = 0, runs = 0, junction_into_tail = 0;
    const uint8_t tails[][2] = {{0, 0}, {0, 1}, {1, 0}, {0, 3}, {0xff, 0}};
    for (int i = 0; i < 300000; i++) {
      tinhat::Iv iv = *a.Next();
      for (uint8_t b : iv) zero_bytes += b == 0;
      low_b0 += iv[0] < 4;
      for (int k : {0, 1, 3, 254}) {
        std::vector<uint8_t> t(iv.begin(), iv.end());
        t.push_back(12);
        t.push_back(static_cast<uint8_t>(k));
        runs += has_escape_run(t);
        for (auto& tl : tails) {
          std::vector<uint8_t> j = {tl[0], tl[1]};
          j.insert(j.end(), t.begin(), t.end());
          // an escape whose third byte is IV[1..] or the trailer would shift the tail; IV[0] as the third byte
          // would put the escape BEFORE the IV (harmless), and IV[0] >= 4 rules even that out
          for (size_t p = 0; p + 2 < j.size(); p++)
            if (j[p] == 0 && j[p + 1] == 0 && j[p + 2] <= 3) junction_into_tail++;
        }
      }
    }
    check(zero_bytes == 0, "no zero byte in 300000 IVs (" + std::to_string(zero_bytes) + ")");
    check(low_b0 == 0, "IV byte 0 >= 4 in every IV (" + std::to_string(low_b0) + " below)");
    check(runs == 0, "no 00 00 [00-03] run inside IV || trailer, key indexes 0/1/3/254 (" + std::to_string(runs) + ")");
    check(junction_into_tail == 0, "no escape run at the tag/IV junction or after it, for the representative tag tails (" + std::to_string(junction_into_tail) + ")");
    // and the escaped tail itself: WriteRbsp(tag tail || IV || trailer) ends in exactly IV || trailer
    int tail_moved = 0;
    tinhat::IvAllocator b(0x00000000u, 0);
    for (int i = 0; i < 20000; i++) {
      tinhat::Iv iv = *b.Next();
      for (auto& tl : tails) {
        std::vector<uint8_t> j = {tl[0], tl[1]};
        j.insert(j.end(), iv.begin(), iv.end());
        j.push_back(12);
        j.push_back(static_cast<uint8_t>(i % 255));
        const std::vector<uint8_t> e = write_rbsp(j);
        if (!std::equal(e.end() - 14, e.end(), j.end() - 14)) tail_moved++;
      }
    }
    check(tail_moved == 0, "the escaped frame tail equals IV || trailer exactly (" + std::to_string(tail_moved) + " moved)");
  }
  // 8. The base-255 digit carries: around every power of 255 (and the limit) the encoding is zero-free, strictly
  //    increasing in byte order, decodes back, and 100000 values across 255^2 and 255^4 are all distinct.
  {
    int bad = 0, ran = 0, ran_at_limit = 0;
    for (int k = 0; k <= 8; k++) {
      uint64_t p = 1;
      for (int j = 0; j < k; j++) p *= 255;  // 255^8 still fits in 64 bits
      std::vector<tinhat::Iv> seq;
      for (int d = -3; d <= 3; d++) {
        // unsigned arithmetic only (255^8 doesn't fit an int64_t): skip p - 3.. below 0 and values >= the limit
        if (d < 0 && p < static_cast<uint64_t>(-d)) continue;
        const uint64_t c = d < 0 ? p - static_cast<uint64_t>(-d) : p + static_cast<uint64_t>(d);
        if (c >= tinhat::IvAllocator::kCounterLimit) continue;
        ran++;
        if (k == 8) ran_at_limit++;
        tinhat::IvAllocator a(0x12345678u, c);
        tinhat::Iv iv = *a.Next();
        for (int i = 4; i < 12; i++) bad += iv[i] == 0;
        bad += ctr_of(iv) != c;
        if (!seq.empty() && !(std::lexicographical_compare(seq.back().begin(), seq.back().end(), iv.begin(), iv.end()))) bad++;
        seq.push_back(iv);
      }
    }
    check(bad == 0 && ran_at_limit == 3 && ran >= 50, "digit carries 255^0..255^8: zero-free, order-preserving, decodes back (" + std::to_string(bad) + " bad, " + std::to_string(ran) + " values, " + std::to_string(ran_at_limit) + " just below 255^8)");
    std::unordered_set<std::string> s2;
    for (uint64_t base : {255ull * 255, 255ull * 255 * 255 * 255}) {
      tinhat::IvAllocator a(0x01020304u, base - 50000);
      for (int i = 0; i < 100000; i++) s2.insert(hex(*a.Next()));
    }
    check(s2.size() == 200000, "200000 IVs across the 255^2 and 255^4 carries are all distinct (" + std::to_string(s2.size()) + ")");
    // the shared vectors' boundary rows (testvectors/iv-layout/vectors.json; the Rust test checks the whole file)
    uint8_t out[8];
    tinhat::IvAllocator::EncodeCounter(0, out);
    bool v = std::all_of(out, out + 8, [](uint8_t b) { return b == 1; });
    tinhat::IvAllocator::EncodeCounter(254, out);
    v = v && out[7] == 255 && out[6] == 1;
    tinhat::IvAllocator::EncodeCounter(255, out);
    v = v && out[7] == 1 && out[6] == 2;
    tinhat::IvAllocator::EncodeCounter(tinhat::IvAllocator::kCounterLimit - 1, out);
    v = v && std::all_of(out, out + 8, [](uint8_t b) { return b == 255; });
    check(v, "boundary vectors 0, 254, 255, 255^8-1");
  }
  // 9. The H.264 round trip, as every receiver in users' hands reads it (raw-tail IV, then unescape, then GCM): IVs
  //    from the allocator always come back; the 12.1.8 v1 layout (big-endian counter, top bytes 00 00 00) fails.
  {
    std::mt19937 rng(1218);
    uint8_t key[16];
    RAND_bytes(key, 16);
    tinhat::IvAllocator a(0x5a5a5a5au, 4228250625ull - 2000);  // across the 255^4 carry too
    int ok = 0, n = 20000;
    for (int i = 0; i < n; i++) ok += h264_round_trip(key, *a.Next(), rng);
    check(ok == n, "H.264 escape -> raw-tail read -> unescape -> decrypt: " + std::to_string(ok) + "/" + std::to_string(n) + " frames with v2 IVs");
    int v1_ok = 0;
    for (int i = 0; i < n; i++) {
      tinhat::Iv iv;
      const uint32_t dom = 0x5a5a5a5au;
      const uint64_t c = (static_cast<uint64_t>(rng() & 0xff) << 32) | rng();
      for (int k = 0; k < 4; k++) iv[k] = static_cast<uint8_t>(dom >> (24 - 8 * k));
      for (int k = 0; k < 8; k++) iv[4 + k] = static_cast<uint8_t>(c >> (56 - 8 * k));
      v1_ok += h264_round_trip(key, iv, rng);
    }
    check(v1_ok == 0, "control: the 12.1.8 v1 layout fails the same round trip on every frame (" + std::to_string(v1_ok) + "/" + std::to_string(n) + " came back)");
  }
  // 10. The hardened receiver (tinhat-iv-rx-v1): unescape first, SplitRxTail as shipped. Every layout comes back,
  //     v1 included (the frames v1 senders put on the wire were always well-formed); short bodies are refused.
  {
    std::mt19937 rng(1219);
    uint8_t key[16];
    RAND_bytes(key, 16);
    tinhat::IvAllocator a(0xa5a5a5a5u, 4228250625ull - 2000);
    int v2_ok = 0, v1_ok = 0, planted_ok = 0, n = 20000;
    for (int i = 0; i < n; i++) v2_ok += h264_round_trip(key, *a.Next(), rng, true);
    for (int i = 0; i < n; i++) {
      tinhat::Iv iv;
      const uint64_t c = (static_cast<uint64_t>(rng() & 0xff) << 32) | rng();
      for (int k = 0; k < 4; k++) iv[k] = static_cast<uint8_t>(0x5a5a5a5au >> (24 - 8 * k));
      for (int k = 0; k < 8; k++) iv[4 + k] = static_cast<uint8_t>(c >> (56 - 8 * k));
      v1_ok += h264_round_trip(key, iv, rng, true);
    }
    int planted_escaped = 0;
    for (int i = 0; i < 1100; i++) {  // `00 00 b` (b <= 3) planted at every IV position
      tinhat::Iv iv;
      for (auto& b : iv) b = static_cast<uint8_t>(1 + rng() % 255);
      const int p = i % 11;
      iv[p] = iv[p + 1] = 0;
      if (p + 2 < 12) { iv[p + 2] = static_cast<uint8_t>((i / 11) % 4); planted_escaped++; }
      planted_ok += h264_round_trip(key, iv, rng, true);
    }
    check(v2_ok == n, "hardened receiver: v2 IVs " + std::to_string(v2_ok) + "/" + std::to_string(n));
    check(v1_ok == n, "hardened receiver: the 12.1.8 v1 layout " + std::to_string(v1_ok) + "/" + std::to_string(n) + " (raw-tail receivers: 0)");
    check(planted_ok == 1100, "hardened receiver: planted 00 00 [00-03] at every IV position " + std::to_string(planted_ok) + "/1100 (" + std::to_string(planted_escaped) + " escaped inside the IV)");
    tinhat::RxTail t;
    bool short_ok = true;
    std::vector<uint8_t> body(64, 0x11);
    for (size_t len = 0; len < 30; len++) {  // < 16 (tag) + 12 (IV) + 2 (trailer)
      if (len >= 2) { body[len - 2] = 12; body[len - 1] = 0; }
      short_ok = short_ok && !tinhat::SplitRxTail(body.data(), len, 12, tinhat::kGcmTagLen, &t);
    }
    body[28] = 12; body[29] = 3;
    short_ok = short_ok && tinhat::SplitRxTail(body.data(), 30, 12, tinhat::kGcmTagLen, &t) && t.ct_len == 16 &&
               t.iv_off == 16 && t.iv_len == 12 && t.key_index == 3;
    body[28] = 16;
    short_ok = short_ok && !tinhat::SplitRxTail(body.data(), 30, 12, tinhat::kGcmTagLen, &t) &&
               !tinhat::SplitRxTail(nullptr, 30, 12, tinhat::kGcmTagLen, &t);
    check(short_ok, "SplitRxTail refuses bodies shorter than tag || IV || trailer, another IV length, null; splits 30 bytes exactly");
  }
  std::printf(fails ? "TINHAT-IV TEST: FAIL (%d)\n" : "TINHAT-IV TEST: PASS\n", fails);
  return fails ? 1 : 0;
}
