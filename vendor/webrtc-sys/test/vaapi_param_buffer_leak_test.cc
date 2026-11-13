/*
 * Hardware-free test for the VA parameter buffers the VAAPI H.264 encoder
 * wrapper creates on every frame.
 *
 * Since VA-API 0.40 vaRenderPicture() does not take ownership of the buffers
 * handed to it: the application must vaDestroyBuffer() them. Mesa's VA
 * frontend frees a buffer only in vaDestroyBuffer(), so a wrapper that never
 * destroys its per-frame picture / slice / packed-header buffers leaks a few
 * of them (each a ~6 KB driver struct plus its payload) on every encoded
 * frame, more on every IDR. Over a long screen share that is gigabytes.
 *
 * This file stands in for libva: it defines every va* function the wrapper
 * calls, with Mesa's ownership rules (a buffer lives until vaDestroyBuffer,
 * vaRenderPicture/vaEndPicture keep nothing alive and free nothing), and
 * defines VaapiDisplayDrm's three methods so no DRM device is opened. No GPU,
 * no libva and no libwebrtc are needed. Build and run (from webrtc-sys/):
 *
 *   c++ -std=c++20 -O1 -I test/shims -I src/vaapi \
 *       test/vaapi_param_buffer_leak_test.cc \
 *       src/vaapi/vaapi_h264_encoder_wrapper.cpp \
 *       -o /tmp/vaapi_param_buffer_leak_test && /tmp/vaapi_param_buffer_leak_test
 *
 * Modes:
 *   (none)                      the gate: PASS/FAIL, exit 1 on FAIL
 *   --rss FRAMES IDR_EVERY      encode FRAMES frames of 2560x1072, an IDR every
 *                               IDR_EVERY frames (0 = only the encoder's own
 *                               period), print RSS every 1000 frames. The fake
 *                               driver allocates what Mesa's frontend does per
 *                               buffer (struct + payload), so the slope is a
 *                               model of the radeonsi case, not a measurement
 *                               of a real driver.
 */

#include <va/va.h>
#include <va/va_enc_h264.h>

#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <algorithm>
#include <cstring>
#include <map>
#include <string>
#include <vector>

#include <unistd.h>

#include "vaapi_display_drm.h"
#include "vaapi_h264_encoder_wrapper.h"

namespace {

// sizeof(vlVaBuffer) in Mesa's frontends/va on x86-64 is about 6.3 KB (it
// embeds per-type state). Allocated for real so RSS reflects it.
constexpr size_t kDriverBufferStruct = 6304;

struct FakeBuffer {
  VABufferType type;
  unsigned size;
  unsigned num;
  void* storage;  // kDriverBufferStruct + payload, like Mesa's struct + data
  void* data;
};

std::map<VABufferID, FakeBuffer> g_buffers;
VABufferID g_next_buffer = 0x1000;
std::map<VASurfaceID, std::vector<uint8_t>> g_surfaces;
VASurfaceID g_next_surface = 0x100;
std::map<VAImageID, VABufferID> g_images;
VAImageID g_next_image = 0x10;
bool g_context_live = false, g_config_live = false;
uint64_t g_param_created = 0, g_param_destroyed = 0;
// protocol errors the wrapper must never make: destroying an unknown buffer
// (a double destroy), or a parameter buffer of the open picture before
// vaEndPicture()
uint64_t g_invalid_destroys = 0, g_destroyed_in_picture = 0;
bool g_in_picture = false;
// failure injection: fail the Nth vaCreateBuffer of a parameter buffer / the
// Nth vaEndPicture (counted from 1; 0 = never)
uint64_t g_fail_param_create_at = 0, g_param_create_calls = 0;
uint64_t g_fail_end_at = 0, g_end_calls = 0;
int g_width = 0, g_height = 0;
std::vector<uint8_t> g_coded_payload;  // what a coded buffer "holds"
VACodedBufferSegment g_segment;
int g_fake_display_tag;

bool is_param_type(VABufferType t) {
  return t != VAEncCodedBufferType && t != VAImageBufferType;
}

size_t live_param_buffers() {
  size_t n = 0;
  for (auto& [id, b] : g_buffers)
    if (is_param_type(b.type)) ++n;
  return n;
}

size_t live_buffers_of(VABufferType t) {
  size_t n = 0;
  for (auto& [id, b] : g_buffers)
    if (b.type == t) ++n;
  return n;
}

VABufferID new_buffer(VABufferType type, unsigned size, unsigned num,
                      const void* src) {
  FakeBuffer b{type, size, num, nullptr, nullptr};
  size_t payload = (type == VAEncCodedBufferType)
                       ? sizeof(VACodedBufferSegment)
                       : size_t(size) * num;
  b.storage = calloc(1, kDriverBufferStruct + payload);
  if (!b.storage) abort();
  b.data = static_cast<uint8_t*>(b.storage) + kDriverBufferStruct;
  if (src && type != VAEncCodedBufferType) memcpy(b.data, src, payload);
  VABufferID id = g_next_buffer++;
  g_buffers[id] = b;
  if (is_param_type(type)) ++g_param_created;
  return id;
}

long rss_kb() {
  FILE* f = fopen("/proc/self/statm", "r");
  long pages = 0, resident = 0;
  if (f) {
    if (fscanf(f, "%ld %ld", &pages, &resident) != 2) resident = 0;
    fclose(f);
  }
  return resident * (sysconf(_SC_PAGESIZE) / 1024);
}

}  // namespace

// ── the VaapiDisplayDrm stand-in (no DRM device) ───────────────────────────
namespace livekit_ffi {
bool VaapiDisplayDrm::Open() {
  va_display_ = &g_fake_display_tag;
  drm_fd_ = -1;
  return true;
}
bool VaapiDisplayDrm::isOpen() const { return va_display_ != nullptr; }
void VaapiDisplayDrm::Close() { va_display_ = nullptr; }
}  // namespace livekit_ffi

// ── the libva stand-in ─────────────────────────────────────────────────────
extern "C" {

VAStatus vaInitialize(VADisplay, int* major, int* minor) {
  *major = 1;
  *minor = 22;
  return VA_STATUS_SUCCESS;
}
VAStatus vaTerminate(VADisplay) { return VA_STATUS_SUCCESS; }
int vaMaxNumEntrypoints(VADisplay) { return 4; }
VAStatus vaQueryConfigEntrypoints(VADisplay, VAProfile, VAEntrypoint* list,
                                  int* num) {
  list[0] = VAEntrypointVLD;
  list[1] = VAEntrypointEncSlice;
  *num = 2;
  return VA_STATUS_SUCCESS;
}
VAStatus vaGetConfigAttributes(VADisplay, VAProfile, VAEntrypoint,
                               VAConfigAttrib* list, int num) {
  for (int i = 0; i < num; i++) {
    switch (list[i].type) {
      case VAConfigAttribRTFormat:
        list[i].value = VA_RT_FORMAT_YUV420;
        break;
      case VAConfigAttribRateControl:
        list[i].value = VA_RC_CQP | VA_RC_CBR | VA_RC_VBR;
        break;
      case VAConfigAttribEncPackedHeaders:  // what radeonsi advertises
        list[i].value = VA_ENC_PACKED_HEADER_SEQUENCE |
                        VA_ENC_PACKED_HEADER_PICTURE |
                        VA_ENC_PACKED_HEADER_SLICE;
        break;
      case VAConfigAttribEncMaxRefFrames:
        list[i].value = 1 | (1 << 16);
        break;
      case VAConfigAttribEncMaxSlices:
        list[i].value = 1;
        break;
      default:
        list[i].value = VA_ATTRIB_NOT_SUPPORTED;
    }
  }
  return VA_STATUS_SUCCESS;
}
VAStatus vaCreateConfig(VADisplay, VAProfile, VAEntrypoint, VAConfigAttrib*,
                        int, VAConfigID* id) {
  g_config_live = true;
  *id = 1;
  return VA_STATUS_SUCCESS;
}
VAStatus vaDestroyConfig(VADisplay, VAConfigID) {
  g_config_live = false;
  return VA_STATUS_SUCCESS;
}
VAStatus vaCreateSurfaces(VADisplay, unsigned int, unsigned int width,
                          unsigned int height, VASurfaceID* surfaces,
                          unsigned int num, VASurfaceAttrib*, unsigned int) {
  g_width = width;
  g_height = height;
  for (unsigned i = 0; i < num; i++) {
    surfaces[i] = g_next_surface++;
    g_surfaces[surfaces[i]];  // pixels allocated lazily on first derive
  }
  return VA_STATUS_SUCCESS;
}
VAStatus vaDestroySurfaces(VADisplay, VASurfaceID* surfaces, int num) {
  for (int i = 0; i < num; i++) g_surfaces.erase(surfaces[i]);
  return VA_STATUS_SUCCESS;
}
VAStatus vaCreateContext(VADisplay, VAConfigID, int, int, int, VASurfaceID*,
                         int, VAContextID* id) {
  g_context_live = true;
  *id = 2;
  return VA_STATUS_SUCCESS;
}
VAStatus vaDestroyContext(VADisplay, VAContextID) {
  g_context_live = false;
  return VA_STATUS_SUCCESS;
}
VAStatus vaCreateBuffer(VADisplay, VAContextID, VABufferType type,
                        unsigned int size, unsigned int num, void* data,
                        VABufferID* id) {
  if (is_param_type(type) && ++g_param_create_calls == g_fail_param_create_at)
    return VA_STATUS_ERROR_ALLOCATION_FAILED;
  *id = new_buffer(type, size, num, data);
  return VA_STATUS_SUCCESS;
}
VAStatus vaDestroyBuffer(VADisplay, VABufferID id) {
  auto it = g_buffers.find(id);
  if (it == g_buffers.end()) {
    ++g_invalid_destroys;
    return VA_STATUS_ERROR_INVALID_BUFFER;
  }
  if (is_param_type(it->second.type)) {
    ++g_param_destroyed;
    if (g_in_picture) ++g_destroyed_in_picture;
  }
  free(it->second.storage);
  g_buffers.erase(it);
  return VA_STATUS_SUCCESS;
}
VAStatus vaMapBuffer(VADisplay, VABufferID id, void** p) {
  auto it = g_buffers.find(id);
  if (it == g_buffers.end()) return VA_STATUS_ERROR_INVALID_BUFFER;
  if (it->second.type == VAEncCodedBufferType) {
    g_segment = {};
    g_segment.size = g_coded_payload.size();
    g_segment.buf = g_coded_payload.data();
    *p = &g_segment;
  } else {
    *p = it->second.data;
  }
  return VA_STATUS_SUCCESS;
}
VAStatus vaUnmapBuffer(VADisplay, VABufferID) { return VA_STATUS_SUCCESS; }
VAStatus vaDeriveImage(VADisplay, VASurfaceID surface, VAImage* image) {
  auto it = g_surfaces.find(surface);
  if (it == g_surfaces.end()) return VA_STATUS_ERROR_INVALID_SURFACE;
  size_t luma = size_t(g_width) * g_height;
  memset(image, 0, sizeof(*image));
  image->format.fourcc = VA_FOURCC_NV12;
  image->width = g_width;
  image->height = g_height;
  image->num_planes = 2;
  image->pitches[0] = image->pitches[1] = g_width;
  image->offsets[1] = luma;
  image->data_size = luma * 3 / 2;
  image->buf = new_buffer(VAImageBufferType, image->data_size, 1, nullptr);
  image->image_id = g_next_image++;
  g_images[image->image_id] = image->buf;
  return VA_STATUS_SUCCESS;
}
VAStatus vaCreateImage(VADisplay, VAImageFormat*, int, int, VAImage*) {
  return VA_STATUS_ERROR_UNIMPLEMENTED;
}
VAStatus vaDestroyImage(VADisplay d, VAImageID id) {
  auto it = g_images.find(id);
  if (it == g_images.end()) return VA_STATUS_ERROR_INVALID_IMAGE;
  vaDestroyBuffer(d, it->second);
  g_images.erase(it);
  return VA_STATUS_SUCCESS;
}
VAStatus vaBeginPicture(VADisplay, VAContextID, VASurfaceID) {
  g_in_picture = true;
  return VA_STATUS_SUCCESS;
}
// Mesa: vaRenderPicture copies what it needs and keeps nothing alive.
VAStatus vaRenderPicture(VADisplay, VAContextID, VABufferID* ids, int num) {
  for (int i = 0; i < num; i++)
    if (!g_buffers.count(ids[i])) return VA_STATUS_ERROR_INVALID_BUFFER;
  return VA_STATUS_SUCCESS;
}
VAStatus vaEndPicture(VADisplay, VAContextID) {
  g_in_picture = false;  // the picture is over either way
  if (++g_end_calls == g_fail_end_at) return VA_STATUS_ERROR_OPERATION_FAILED;
  return VA_STATUS_SUCCESS;
}
VAStatus vaSyncSurface(VADisplay, VASurfaceID) { return VA_STATUS_SUCCESS; }

}  // extern "C"

// ── the test ────────────────────────────────────────────────────────────────
namespace {

constexpr int kW = 2560, kH = 1072, kFps = 30;
int g_failures = 0;

void check(bool ok, const std::string& what) {
  printf("%s: %s\n", ok ? "ok  " : "FAIL", what.c_str());
  if (!ok) ++g_failures;
}

struct Frame {
  std::vector<uint8_t> y, u, v;
  Frame() : y(kW * kH, 16), u(kW * kH / 4, 128), v(kW * kH / 4, 128) {}
};

bool init(livekit_ffi::VaapiH264EncoderWrapper& enc) {
  // the arguments h264_encoder_impl.cpp passes: key frame interval 5 s
  return enc.Initialize(kW, kH, 12000 * 1000, kFps * 5, kFps * 5, 1, kFps,
                        VAProfileH264High, VA_RC_CBR);
}

int gate() {
  g_coded_payload.assign(48 * 1024, 0x42);  // ~one 11.6 Mbit/s 30 fps frame
  Frame f;
  std::vector<uint8_t> out;
  livekit_ffi::VaapiH264EncoderWrapper enc;
  check(init(enc), "Initialize() succeeds against the fake driver");
  const size_t coded_after_init = live_buffers_of(VAEncCodedBufferType);
  check(coded_after_init == SURFACE_NUM,
        "Initialize() creates one coded buffer per surface (" +
            std::to_string(coded_after_init) + ")");

  // 300 frames = 10 s at 30 fps; a forced IDR every 6 s like the reported PLI
  // rate, plus the encoder's own 5 s period.
  size_t worst_live = 0;
  int worst_frame = -1;
  bool encoded_all = true;
  for (int i = 0; i < 300; i++) {
    bool idr = (i % 180) == 0;
    encoded_all &= enc.Encode(VA_FOURCC_I420, f.y.data(), f.u.data(),
                              f.v.data(), idr, out);
    size_t live = live_param_buffers();
    if (live > worst_live) {
      worst_live = live;
      worst_frame = i;
    }
  }
  check(encoded_all, "300 frames encoded");
  // radeonsi-like packed headers: picture + slice + packed slice pair = 4 per
  // frame, 6 more on an IDR; the wrapper must really create them (a test that
  // allocates nothing would pass vacuously)
  check(g_param_created >= 300 * 4,
        "the wrapper creates its per-frame parameter buffers (" +
            std::to_string(g_param_created) + " for 300 frames)");
  check(out.size() == g_coded_payload.size(),
        "Encode() returns the coded payload (" + std::to_string(out.size()) +
            " bytes)");
  check(worst_live == 0,
        "no parameter buffer outlives Encode() (worst: " +
            std::to_string(worst_live) + " live after frame " +
            std::to_string(worst_frame) + "; created " +
            std::to_string(g_param_created) + ", destroyed " +
            std::to_string(g_param_destroyed) + ")");
  check(live_buffers_of(VAEncCodedBufferType) == SURFACE_NUM,
        "coded buffers are reused, not re-created per frame");
  check(g_images.empty(), "every derived image is destroyed");

  enc.Destroy();
  check(g_buffers.empty(),
        "Destroy() leaves no VA buffer (" + std::to_string(g_buffers.size()) +
            " left)");
  check(!g_context_live && !g_config_live && g_surfaces.empty(),
        "Destroy() destroys the context, config and surfaces");

  // a second session on the same wrapper object (a re-publish)
  g_param_created = g_param_destroyed = 0;
  check(init(enc), "re-Initialize() after Destroy()");
  bool encoded_all2 = true;
  for (int i = 0; i < 60; i++)
    encoded_all2 &= enc.Encode(VA_FOURCC_I420, f.y.data(), f.u.data(),
                               f.v.data(), i == 0, out);
  check(encoded_all2 && g_param_created >= 60 * 4,
        "second session: 60 frames encoded with their parameter buffers");
  check(live_param_buffers() == 0,
        "second session: no parameter buffer outlives Encode() (" +
            std::to_string(live_param_buffers()) + " live)");
  enc.Destroy();
  check(g_buffers.empty(), "second session: Destroy() leaves no VA buffer");

  // failures mid-picture: a parameter buffer that can't be created (on an IDR
  // and on a P frame), and a vaEndPicture() that fails (IDR and P). Whatever
  // was created for that picture must still be gone when Encode() returns.
  struct Fault {
    const char* what;
    uint64_t* at;
    uint64_t value;
  } faults[] = {
      {"IDR frame, a parameter buffer can't be created",
       &g_fail_param_create_at, 3},
      {"P frame, a parameter buffer can't be created",
       &g_fail_param_create_at, 10 + 4 * 5 + 2},
      {"IDR frame, vaEndPicture() fails", &g_fail_end_at, 1},
      {"P frame, vaEndPicture() fails", &g_fail_end_at, 6},
  };
  for (auto& fault : faults) {
    g_param_create_calls = g_end_calls = 0;
    g_fail_param_create_at = g_fail_end_at = 0;
    *fault.at = fault.value;
    check(init(enc), std::string(fault.what) + ": Initialize()");
    size_t worst = 0;
    for (int i = 0; i < 12; i++) {
      enc.Encode(VA_FOURCC_I420, f.y.data(), f.u.data(), f.v.data(), i == 0,
                 out);
      worst = std::max(worst, live_param_buffers());
    }
    check(worst == 0, std::string(fault.what) +
                          ": no parameter buffer outlives Encode() (worst " +
                          std::to_string(worst) + ")");
    enc.Destroy();
    check(g_buffers.empty(), std::string(fault.what) + ": Destroy() leaves no VA buffer");
  }
  g_fail_param_create_at = g_fail_end_at = 0;

  check(g_invalid_destroys == 0,
        "no buffer is destroyed twice or unknown (" +
            std::to_string(g_invalid_destroys) + ")");
  check(g_destroyed_in_picture == 0,
        "no parameter buffer is destroyed before vaEndPicture() (" +
            std::to_string(g_destroyed_in_picture) + ")");

  printf("%s (%d failure(s))\n", g_failures ? "FAIL" : "PASS", g_failures);
  return g_failures ? 1 : 0;
}

int rss(int frames, int idr_every) {
  if (frames < 1000 || idr_every < 0) {
    fprintf(stderr, "--rss FRAMES IDR_EVERY: FRAMES >= 1000, IDR_EVERY >= 0\n");
    return 2;
  }
  g_coded_payload.assign(48 * 1024, 0x42);
  Frame f;
  std::vector<uint8_t> out;
  livekit_ffi::VaapiH264EncoderWrapper enc;
  if (!init(enc)) return 2;
  long base = 0;
  printf("frame rss_kb live_param_buffers\n");
  for (int i = 0; i <= frames; i++) {
    bool idr = idr_every > 0 && (i % idr_every) == 0;
    if (!enc.Encode(VA_FOURCC_I420, f.y.data(), f.u.data(), f.v.data(), idr, out))
      return 3;
    if (i == 100) base = rss_kb();
    if (i % 1000 == 0)
      printf("%d %ld %zu\n", i, rss_kb(), live_param_buffers());
  }
  long end = rss_kb();
  double kb_per_frame = double(end - base) / double(frames - 100);
  printf("slope %.2f KB/frame = %.1f MB/min at 30 fps (%d frames, IDR every %d)\n",
         kb_per_frame, kb_per_frame * 30 * 60 / 1024.0, frames, idr_every);
  enc.Destroy();
  return 0;
}

}  // namespace

int main(int argc, char** argv) {
  if (argc >= 2 && std::string(argv[1]) == "--rss") {
    int frames = argc >= 3 ? atoi(argv[2]) : 20000;
    int idr_every = argc >= 4 ? atoi(argv[3]) : 0;
    return rss(frames, idr_every);
  }
  return gate();
}
