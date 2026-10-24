#include "nvidia_encoder_factory.h"

#include <memory>

#include "cuda_context.h"
#include "av1_encoder_impl.h"
#include "h264_encoder_impl.h"
#include "h265_encoder_impl.h"
#include "nvEncodeAPI.h"
#include "rtc_base/logging.h"

// PATCH (Tin Hat, windows-port): the availability probe below dlopens the
// driver's NVENC runtime. On Windows that library is nvEncodeAPI64.dll (same
// role as libnvidia-encode.so.1) and the loader API is LoadLibrary/GetProcAddress.
#if defined(_WIN32)
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#else
#include <dlfcn.h>
#endif

namespace webrtc {

NvidiaVideoEncoderFactory::NvidiaVideoEncoderFactory() {
  std::map<std::string, std::string> baselineParameters = {
      {"profile-level-id", "42e01f"},
      {"level-asymmetry-allowed", "1"},
      {"packetization-mode", "1"},
  };
  supported_formats_.push_back(SdpVideoFormat("H264", baselineParameters));

  // Advertise HEVC/H265 with default parameters.
  supported_formats_.push_back(SdpVideoFormat("H265"));
  // Some stacks use 'HEVC' name.
  supported_formats_.push_back(SdpVideoFormat("HEVC"));

  // PATCH (Tin Hat, NVENC AV1): AV1, only when this GPU's NVENC encodes it
  // (RTX 40/50-series; H.264 hard-caps at 4096x4096, AV1 reaches 8192).
  if (SupportsAv1()) {
    supported_formats_.push_back(SdpVideoFormat::AV1Profile0());
  }

  /*std::map<std::string, std::string> highParameters = {
      {"profile-level-id", "4d0032"},
      {"level-asymmetry-allowed", "1"},
      {"packetization-mode", "1"},
  };

  supported_formats_.push_back(SdpVideoFormat("H264", highParameters));
  */
}

NvidiaVideoEncoderFactory::~NvidiaVideoEncoderFactory() {}

// PATCH (Tin Hat, NVENC AV1): does this GPU's NVENC encode AV1? Opens a
// transient session and asks for the AV1 max width — present only on RTX 40+
// (Ada/Blackwell). 0.8.20: only a POSITIVE answer is cached. The old
// code cached the first answer either way, so one transient failure (a busy
// NVENC session table, a CUDA context that wasn't ready yet) pinned every later
// share in the process to H.264 on a machine that did AV1 the day before — with
// no line in the app log to say so. Negatives now re-probe (a share publish is
// rare) and print the failing step to stderr (the app's log file).
bool NvidiaVideoEncoderFactory::SupportsAv1() {
  static int cached = -1;
  if (cached == 1) return true;
  const char* why = "unknown";
  int result = 0;
  do {
    if (!livekit_ffi::CudaContext::IsAvailable()) { why = "CUDA context unavailable"; break; }
#if defined(_WIN32)
    HMODULE hModule = LoadLibraryA("nvEncodeAPI64.dll");
    if (!hModule) { why = "nvEncodeAPI64.dll not loadable"; break; }
    auto CreateInstance =
        (NVENCSTATUS(NVENCAPI*)(NV_ENCODE_API_FUNCTION_LIST*))GetProcAddress(
            hModule, "NvEncodeAPICreateInstance");
#else
    void* hModule = dlopen("libnvidia-encode.so.1", RTLD_LAZY);
    if (!hModule) { why = "libnvidia-encode.so.1 not loadable"; break; }
    auto CreateInstance =
        (NVENCSTATUS(NVENCAPI*)(NV_ENCODE_API_FUNCTION_LIST*))dlsym(
            hModule, "NvEncodeAPICreateInstance");
#endif
    if (!CreateInstance) { why = "NvEncodeAPICreateInstance symbol missing"; break; }
    NV_ENCODE_API_FUNCTION_LIST fnList = {NV_ENCODE_API_FUNCTION_LIST_VER};
    if (CreateInstance(&fnList) != NV_ENC_SUCCESS) { why = "NvEncodeAPICreateInstance failed (driver/API version?)"; break; }
    if (cuInit(0) != CUDA_SUCCESS) { why = "cuInit failed"; break; }
    CUdevice dev;
    if (cuDeviceGet(&dev, 0) != CUDA_SUCCESS) { why = "cuDeviceGet(0) failed"; break; }
    CUcontext ctx = nullptr;
#if CUDA_VERSION >= 13000
    if (cuCtxCreate(&ctx, nullptr, 0, dev) != CUDA_SUCCESS || !ctx) { why = "cuCtxCreate failed"; break; }
#else
    if (cuCtxCreate(&ctx, 0, dev) != CUDA_SUCCESS || !ctx) { why = "cuCtxCreate failed"; break; }
#endif
    NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS sp = {
        NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS_VER};
    sp.device = ctx;
    sp.deviceType = NV_ENC_DEVICE_TYPE_CUDA;
    sp.apiVersion = NVENCAPI_VERSION;
    void* enc = nullptr;
    NVENCSTATUS os = fnList.nvEncOpenEncodeSessionEx(&sp, &enc);
    if (os == NV_ENC_SUCCESS && enc) {
      NV_ENC_CAPS_PARAM cp = {NV_ENC_CAPS_PARAM_VER};
      cp.capsToQuery = NV_ENC_CAPS_WIDTH_MAX;
      int w = 0;
      NVENCSTATUS cs = fnList.nvEncGetEncodeCaps(enc, NV_ENC_CODEC_AV1_GUID, &cp, &w);
      if (cs == NV_ENC_SUCCESS && w > 0) {
        result = 1;
        RTC_LOG(LS_INFO) << "NVENC AV1 available, max width " << w;
        fprintf(stderr, "[nvenc-av1] probe: available (max width %d)\n", w);
      } else {
        static char buf[96];
        snprintf(buf, sizeof(buf), "nvEncGetEncodeCaps(AV1) status=%d maxWidth=%d", (int)cs, w);
        why = buf;
      }
      fnList.nvEncDestroyEncoder(enc);
    } else {
      static char buf2[96];
      snprintf(buf2, sizeof(buf2), "nvEncOpenEncodeSessionEx status=%d (session table full? driver?)", (int)os);
      why = buf2;
    }
    cuCtxDestroy(ctx);
  } while (false);
  if (result == 1) {
    cached = 1;
  } else {
    RTC_LOG(LS_INFO) << "NVENC AV1 not available on this probe: " << why;
    fprintf(stderr, "[nvenc-av1] probe: NOT available (%s) — not cached, re-probed on the next share\n", why);
  }
  return result == 1;
}

bool NvidiaVideoEncoderFactory::IsSupported() {
  if (!livekit_ffi::CudaContext::IsAvailable()) {
    RTC_LOG(LS_WARNING) << "CUDA is not available, NVENC disabled.";
    return false;
  }

  // CUDA being available does NOT imply NVENC is present. Compute-only GPUs
  // (H100, A100, etc.) have full CUDA support but no encode hardware.
  // Probe the NVENC library and try to open a session to be sure.
#if defined(_WIN32)
  HMODULE hModule = LoadLibraryA("nvEncodeAPI64.dll");
  if (!hModule) {
    RTC_LOG(LS_WARNING) << "NVENC library (nvEncodeAPI64.dll) not found, "
                           "hardware encoding unavailable.";
    return false;
  }
  auto NvEncodeAPIGetMaxSupportedVersion =
      (NVENCSTATUS(NVENCAPI*)(uint32_t*))GetProcAddress(
          hModule, "NvEncodeAPIGetMaxSupportedVersion");
  auto NvEncodeAPICreateInstance =
      (NVENCSTATUS(NVENCAPI*)(NV_ENCODE_API_FUNCTION_LIST*))GetProcAddress(
          hModule, "NvEncodeAPICreateInstance");
#else
  void* hModule = dlopen("libnvidia-encode.so.1", RTLD_LAZY);
  if (!hModule) {
    RTC_LOG(LS_WARNING) << "NVENC library (libnvidia-encode.so.1) not found, "
                           "hardware encoding unavailable.";
    return false;
  }
  auto NvEncodeAPIGetMaxSupportedVersion =
      (NVENCSTATUS(NVENCAPI*)(uint32_t*))dlsym(
          hModule, "NvEncodeAPIGetMaxSupportedVersion");
  auto NvEncodeAPICreateInstance =
      (NVENCSTATUS(NVENCAPI*)(NV_ENCODE_API_FUNCTION_LIST*))dlsym(
          hModule, "NvEncodeAPICreateInstance");
#endif

  bool supported = false;

  do {
    if (!NvEncodeAPIGetMaxSupportedVersion || !NvEncodeAPICreateInstance) {
      RTC_LOG(LS_WARNING) << "NVENC API entry points not found in library.";
      break;
    }

    uint32_t maxVersion = 0;
    if (NvEncodeAPIGetMaxSupportedVersion(&maxVersion) != NV_ENC_SUCCESS) {
      RTC_LOG(LS_WARNING) << "NvEncodeAPIGetMaxSupportedVersion failed.";
      break;
    }

    uint32_t currentVersion =
        (NVENCAPI_MAJOR_VERSION << 4) | NVENCAPI_MINOR_VERSION;
    if (currentVersion > maxVersion) {
      RTC_LOG(LS_WARNING) << "NVENC driver version too old: driver supports "
                          << maxVersion << ", SDK requires " << currentVersion;
      break;
    }

    NV_ENCODE_API_FUNCTION_LIST fnList = {NV_ENCODE_API_FUNCTION_LIST_VER};
    if (NvEncodeAPICreateInstance(&fnList) != NV_ENC_SUCCESS) {
      RTC_LOG(LS_WARNING) << "NvEncodeAPICreateInstance failed.";
      break;
    }

    // Try opening an encode session with CUDA device 0 to confirm the GPU
    // actually has NVENC hardware.
    CUresult cuRes = cuInit(0);
    if (cuRes != CUDA_SUCCESS) {
      RTC_LOG(LS_WARNING) << "cuInit failed during NVENC probe.";
      break;
    }

    CUdevice cuDevice;
    cuRes = cuDeviceGet(&cuDevice, 0);
    if (cuRes != CUDA_SUCCESS) {
      RTC_LOG(LS_WARNING) << "cuDeviceGet failed during NVENC probe.";
      break;
    }

    CUcontext cuCtx = nullptr;
#if CUDA_VERSION >= 13000
    cuRes = cuCtxCreate(&cuCtx, nullptr, 0, cuDevice);
#else
    cuRes = cuCtxCreate(&cuCtx, 0, cuDevice);
#endif
    if (cuRes != CUDA_SUCCESS || !cuCtx) {
      RTC_LOG(LS_WARNING) << "cuCtxCreate failed during NVENC probe.";
      break;
    }

    NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS sessionParams = {
        NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS_VER};
    sessionParams.device = cuCtx;
    sessionParams.deviceType = NV_ENC_DEVICE_TYPE_CUDA;
    sessionParams.apiVersion = NVENCAPI_VERSION;

    void* hEncoder = nullptr;
    NVENCSTATUS nvStatus =
        fnList.nvEncOpenEncodeSessionEx(&sessionParams, &hEncoder);

    if (nvStatus == NV_ENC_SUCCESS && hEncoder) {
      fnList.nvEncDestroyEncoder(hEncoder);
      supported = true;
    } else {
      char deviceName[80] = {};
      cuDeviceGetName(deviceName, sizeof(deviceName), cuDevice);
      RTC_LOG(LS_WARNING) << "NVENC not available on GPU \"" << deviceName
                          << "\" (status=" << nvStatus
                          << "). This GPU likely lacks encode hardware.";
    }

    cuCtxDestroy(cuCtx);
  } while (false);

#if defined(_WIN32)
  FreeLibrary(hModule);
#else
  dlclose(hModule);
#endif

  if (supported) {
    RTC_LOG(LS_INFO) << "NVIDIA NVENC hardware encoder is available.";
  }
  return supported;
}

std::unique_ptr<VideoEncoder> NvidiaVideoEncoderFactory::Create(
    const Environment& env,
    const SdpVideoFormat& format) {
  // Check if the requested format is supported.
  for (const auto& supported_format : supported_formats_) {
    if (format.IsSameCodec(supported_format)) {
      if (!cu_context_) {
        cu_context_ = livekit_ffi::CudaContext::GetInstance();
        if (!cu_context_->Initialize()) {
          RTC_LOG(LS_ERROR) << "Failed to initialize CUDA context.";
          return nullptr;
        }
      }

      if (format.name == "H264") {
        RTC_LOG(LS_INFO) << "Using NVIDIA HW encoder (NVENC) for H264";
        return std::make_unique<NvidiaH264EncoderImpl>(
            env, cu_context_->GetContext(), CU_MEMORYTYPE_DEVICE,
            NV_ENC_BUFFER_FORMAT_IYUV, format);
      }

      if (format.name == "AV1") {
        RTC_LOG(LS_INFO) << "Using NVIDIA HW encoder (NVENC) for AV1";
        return std::make_unique<NvidiaAv1EncoderImpl>(
            env, cu_context_->GetContext(), CU_MEMORYTYPE_DEVICE,
            NV_ENC_BUFFER_FORMAT_IYUV, format);
      }

      if (format.name == "H265" || format.name == "HEVC") {
        RTC_LOG(LS_INFO) << "Using NVIDIA HW encoder (NVENC) for H265/HEVC";
        return std::make_unique<NvidiaH265EncoderImpl>(
            env, cu_context_->GetContext(), CU_MEMORYTYPE_DEVICE,
            NV_ENC_BUFFER_FORMAT_IYUV, format);
      }
    }
  }
  return nullptr;
}
std::vector<SdpVideoFormat> NvidiaVideoEncoderFactory::GetSupportedFormats()
    const {
  return supported_formats_;
}

std::vector<SdpVideoFormat> NvidiaVideoEncoderFactory::GetImplementations()
    const {
  return supported_formats_;
}

}  // namespace webrtc
