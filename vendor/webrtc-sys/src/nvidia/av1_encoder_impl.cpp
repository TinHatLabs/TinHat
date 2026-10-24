#include "av1_encoder_impl.h"

#include <algorithm>
#include <string>

#include "i420_buffer_cuda.h"
#include "common_video/libyuv/include/webrtc_libyuv.h"
#include "modules/video_coding/include/video_codec_interface.h"
#include "modules/video_coding/include/video_error_codes.h"
#include "modules/video_coding/svc/scalable_video_controller_no_layering.h"
#include "modules/video_coding/utility/simulcast_rate_allocator.h"
#include "rtc_base/checks.h"
#include "rtc_base/logging.h"

namespace webrtc {

NvidiaAv1EncoderImpl::NvidiaAv1EncoderImpl(const webrtc::Environment& env,
                                           CUcontext context,
                                           CUmemorytype memory_type,
                                           NV_ENC_BUFFER_FORMAT nv_format,
                                           const SdpVideoFormat& format)
    : env_(env),
      encoder_(nullptr),
      cu_context_(context),
      cu_memory_type_(memory_type),
      nv_format_(nv_format),
      format_(format) {
  RTC_CHECK_NE(cu_memory_type_, CU_MEMORYTYPE_HOST);
}

NvidiaAv1EncoderImpl::~NvidiaAv1EncoderImpl() {
  Release();
}

int32_t NvidiaAv1EncoderImpl::InitEncode(const VideoCodec* inst,
                                         const VideoEncoder::Settings& settings) {
  if (inst) {
    fprintf(stderr, "[nvenc-av1] InitEncode %ux%u@%u\n", inst->width,
            inst->height, inst->maxFramerate);
  }
  if (!inst || inst->codecType != kVideoCodecAV1) {
    return WEBRTC_VIDEO_CODEC_ERR_PARAMETER;
  }
  if (inst->maxFramerate == 0 || inst->width < 1 || inst->height < 1) {
    return WEBRTC_VIDEO_CODEC_ERR_PARAMETER;
  }

  int32_t release_ret = Release();
  if (release_ret != WEBRTC_VIDEO_CODEC_OK) {
    return release_ret;
  }

  codec_ = *inst;

  const size_t new_capacity =
      CalcBufferSize(VideoType::kI420, codec_.width, codec_.height);
  encoded_image_.SetEncodedData(EncodedImageBuffer::Create(new_capacity));
  encoded_image_._encodedWidth = codec_.width;
  encoded_image_._encodedHeight = codec_.height;
  encoded_image_.set_size(0);

  configuration_.sending = false;
  configuration_.max_frame_rate = codec_.maxFramerate;
  configuration_.target_bps = codec_.startBitrate * 1000;
  configuration_.max_bps = codec_.maxBitrate * 1000;

  svc_controller_ = std::make_unique<ScalableVideoControllerNoLayering>();

  const CUresult result = cuCtxSetCurrent(cu_context_);
  if (result != CUDA_SUCCESS) {
    return WEBRTC_VIDEO_CODEC_ENCODER_FAILURE;
  }

  try {
    // PATCH (Tin Hat, AV1 black-tile root cause): NvEncoder's AV1 path defaults to
    // bUseIVFContainer=true, prepending an IVF file header ("DKIF", once) and
    // a 12-byte IVF frame header to EVERY encoded frame. The AV1 RTP
    // packetizer parses those bytes as OBUs — 0xf2 reads as reserved type 14 —
    // and drops whole frames with a compiled-out RTC_DLOG, so NVENC-AV1 over
    // RTP shipped exactly zero decodable frames while logging nothing. RTP
    // wants raw low-overhead OBUs: IVF off. (H264/HEVC never hit this — the
    // IVF branch is AV1-only in NvEncoder::GetEncodedPacket.)
    encoder_ = std::make_unique<NvEncoderCuda>(cu_context_, codec_.width,
                                               codec_.height, nv_format_, 0,
                                               /*bMotionEstimationOnly=*/false,
                                               /*bOPInVideoMemory=*/false,
                                               /*bUseIVFContainer=*/false);
  } catch (const NVENCException& e) {
    fprintf(stderr, "[nvenc-av1] NvEncoderCuda ctor FAILED %ux%u: %s\n",
            codec_.width, codec_.height, e.what());
    return WEBRTC_VIDEO_CODEC_ERROR;
  }

  nv_initialize_params_.version = NV_ENC_INITIALIZE_PARAMS_VER;
  nv_encode_config_.version = NV_ENC_CONFIG_VER;
  nv_initialize_params_.encodeConfig = &nv_encode_config_;

  GUID encodeGuid = NV_ENC_CODEC_AV1_GUID;

  const auto apply_baseline = [&]() {
    nv_initialize_params_.frameRateNum =
        static_cast<uint32_t>(configuration_.max_frame_rate);
    nv_initialize_params_.frameRateDen = 1;
    nv_initialize_params_.bufferFormat = nv_format_;

    nv_encode_config_.profileGUID = NV_ENC_AV1_PROFILE_MAIN_GUID;
    nv_encode_config_.gopLength = NVENC_INFINITE_GOPLENGTH;
    nv_encode_config_.frameIntervalP = 1;
    // The sequence header must ride every keyframe or a mid-stream joiner can
    // never start decoding (AV1's SPS/PPS equivalent).
    nv_encode_config_.encodeCodecConfig.av1Config.repeatSeqHdr = 1;
    nv_encode_config_.encodeCodecConfig.av1Config.idrPeriod =
        NVENC_INFINITE_GOPLENGTH;
    nv_encode_config_.encodeCodecConfig.av1Config.chromaFormatIDC = 1;
    nv_encode_config_.encodeCodecConfig.av1Config.level =
        NV_ENC_LEVEL_AV1_AUTOSELECT;
    // Note: a tile-grid config lived here in 0.7.2/0.7.3 (parallel decode
    // for receivers) and PASSED the 5090-rig loopback at 107% decode rate — but
    // in a field setup the sender collapsed to ~8fps with BWE cratering, on a
    // different NVENC generation. Cause not isolated; the
    // known-good 0.7.1 bitstream is restored EXACTLY (NVENC default = minimum
    // tiles). Re-introduce only as an opt-in experiment (TINHAT_AV1_TILES=CxR)
    // with the [nvenc-av1] encode-ms instrumentation below proving it harmless
    // on the target hardware first.
    if (const char* tiles = getenv("TINHAT_AV1_TILES")) {
      unsigned cols = 0, rows = 0;
      if (sscanf(tiles, "%ux%u", &cols, &rows) == 2 && cols >= 1 && rows >= 1) {
        nv_encode_config_.encodeCodecConfig.av1Config.enableCustomTileConfig = 0;
        nv_encode_config_.encodeCodecConfig.av1Config.numTileColumns = cols;
        nv_encode_config_.encodeCodecConfig.av1Config.numTileRows = rows;
        fprintf(stderr, "[nvenc-av1] EXPERIMENT tile grid %ux%u\n", cols, rows);
      }
    }
    nv_encode_config_.rcParams.version = NV_ENC_RC_PARAMS_VER;
    nv_encode_config_.rcParams.rateControlMode = NV_ENC_PARAMS_RC_CBR;
    nv_encode_config_.rcParams.averageBitRate = configuration_.target_bps;
    nv_encode_config_.rcParams.vbvBufferSize =
        (nv_encode_config_.rcParams.averageBitRate *
         nv_initialize_params_.frameRateDen /
         nv_initialize_params_.frameRateNum) *
        5;
    nv_encode_config_.rcParams.vbvInitialDelay =
        nv_encode_config_.rcParams.vbvBufferSize;
  };

  // Same motion-quality config the H.264 path earned through field reports
  // (P5 + LOW_LATENCY + two-pass + AQ), same graceful fallback to P4/ULL.
  try {
    encoder_->CreateDefaultEncoderParams(&nv_initialize_params_, encodeGuid,
                                         NV_ENC_PRESET_P5_GUID,
                                         NV_ENC_TUNING_INFO_LOW_LATENCY);
    apply_baseline();
    nv_encode_config_.rcParams.multiPass = NV_ENC_TWO_PASS_QUARTER_RESOLUTION;
    nv_encode_config_.rcParams.enableAQ = 1;
    nv_encode_config_.rcParams.aqStrength = 0;
    if (encoder_->GetCapabilityValue(encodeGuid,
                                     NV_ENC_CAPS_SUPPORT_TEMPORAL_AQ)) {
      nv_encode_config_.rcParams.enableTemporalAQ = 1;
    }
    encoder_->CreateEncoder(&nv_initialize_params_);
    fprintf(stderr,
            "[nvenc-av1] init %ux%u@%.0f: P5/LL two-pass spatialAQ%s\n",
            codec_.width, codec_.height, configuration_.max_frame_rate,
            nv_encode_config_.rcParams.enableTemporalAQ ? "+temporalAQ" : "");
  } catch (const NVENCException& e) {
    fprintf(stderr,
            "[nvenc-av1] quality config rejected (%s) — retrying baseline P4/ULL\n",
            e.what());
    try {
      encoder_->CreateDefaultEncoderParams(&nv_initialize_params_, encodeGuid,
                                           NV_ENC_PRESET_P4_GUID,
                                           NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY);
      apply_baseline();
      encoder_->CreateEncoder(&nv_initialize_params_);
    } catch (const NVENCException& e2) {
      fprintf(stderr, "[nvenc-av1] CreateEncoder FAILED %ux%u@%.0f: %s\n",
              codec_.width, codec_.height, configuration_.max_frame_rate,
              e2.what());
      return WEBRTC_VIDEO_CODEC_ERROR;
    }
  }

  RTC_LOG(LS_INFO) << "NVIDIA AV1 NVENC initialized: " << codec_.width << "x"
                   << codec_.height << " @ " << codec_.maxFramerate
                   << "fps, target_bps=" << configuration_.target_bps;

  SimulcastRateAllocator init_allocator(env_, codec_);
  VideoBitrateAllocation allocation =
      init_allocator.Allocate(VideoBitrateAllocationParameters(
          DataRate::KilobitsPerSec(codec_.startBitrate), codec_.maxFramerate));
  SetRates(RateControlParameters(allocation, codec_.maxFramerate));
  return WEBRTC_VIDEO_CODEC_OK;
}

int32_t NvidiaAv1EncoderImpl::RegisterEncodeCompleteCallback(
    EncodedImageCallback* callback) {
  encoded_image_callback_ = callback;
  return WEBRTC_VIDEO_CODEC_OK;
}

int32_t NvidiaAv1EncoderImpl::Release() {
  if (encoder_) {
    encoder_->DestroyEncoder();
    encoder_ = nullptr;
  }
  return WEBRTC_VIDEO_CODEC_OK;
}

int32_t NvidiaAv1EncoderImpl::Encode(
    const VideoFrame& input_frame,
    const std::vector<VideoFrameType>* frame_types) {
  if (!encoder_) {
    return WEBRTC_VIDEO_CODEC_UNINITIALIZED;
  }
  if (!encoded_image_callback_) {
    return WEBRTC_VIDEO_CODEC_UNINITIALIZED;
  }

  webrtc::scoped_refptr<I420BufferInterface> frame_buffer =
      input_frame.video_frame_buffer()->ToI420();
  if (!frame_buffer) {
    RTC_LOG(LS_ERROR) << "Failed to convert frame to I420 for AV1 encode.";
    return WEBRTC_VIDEO_CODEC_ENCODER_FAILURE;
  }

  bool send_key_frame =
      (configuration_.key_frame_request && configuration_.sending) ||
      (frame_types && (*frame_types)[0] == VideoFrameType::kVideoFrameKey);
  if (send_key_frame) {
    configuration_.key_frame_request = false;
  }

  if (!configuration_.sending) {
    return WEBRTC_VIDEO_CODEC_NO_OUTPUT;
  }
  if (frame_types != nullptr &&
      (*frame_types)[0] == VideoFrameType::kEmptyFrame) {
    return WEBRTC_VIDEO_CODEC_NO_OUTPUT;
  }

  // [nvenc-av1-rt] fed = frames given to us, out = packets NVENC returned,
  // enc_ms = wall time inside EncodeFrame. Printed ~every 5s of fed frames.
  static thread_local uint64_t rt_fed = 0, rt_out = 0;
  static thread_local double rt_ms = 0.0;
  static thread_local int64_t rt_last_print = 0;
  rt_fed++;
  const auto rt_t0 = std::chrono::steady_clock::now();
  try {
    const NvEncInputFrame* nv_enc_input_frame = encoder_->GetNextInputFrame();
    if (cu_memory_type_ == CU_MEMORYTYPE_DEVICE) {
      CopyI420BufferToDeviceFrame(cu_context_, *frame_buffer,
                                  *nv_enc_input_frame);
    }

    NV_ENC_PIC_PARAMS pic_params = NV_ENC_PIC_PARAMS();
    pic_params.version = NV_ENC_PIC_PARAMS_VER;
    pic_params.encodePicFlags = 0;
    if (send_key_frame) {
      // No OUTPUT_SPSPPS for AV1 — repeatSeqHdr already rides the keyframe.
      pic_params.encodePicFlags =
          NV_ENC_PIC_FLAG_FORCEINTRA | NV_ENC_PIC_FLAG_FORCEIDR;
    }

    std::vector<std::vector<uint8_t>> bit_stream;
    encoder_->EncodeFrame(bit_stream, &pic_params);
    rt_out += bit_stream.size();
    rt_ms += std::chrono::duration<double, std::milli>(
                 std::chrono::steady_clock::now() - rt_t0)
                 .count();
    const int64_t rt_now = std::chrono::duration_cast<std::chrono::seconds>(
                               std::chrono::steady_clock::now().time_since_epoch())
                               .count();
    if (rt_last_print == 0) rt_last_print = rt_now;
    if (rt_now - rt_last_print >= 5) {
      fprintf(stderr,
              "[nvenc-av1-rt] fed=%llu out=%llu avg_enc_ms=%.1f over %llds\n",
              (unsigned long long)rt_fed, (unsigned long long)rt_out,
              rt_fed ? rt_ms / rt_fed : 0.0, (long long)(rt_now - rt_last_print));
      rt_fed = 0; rt_out = 0; rt_ms = 0.0; rt_last_print = rt_now;
    }

    for (std::vector<uint8_t>& packet : bit_stream) {
      // Keyframe = OUR forced flag ONLY. The first version also trusted a
      // leading OBU_SEQUENCE_HEADER — but NVENC re-emits sequence headers
      // after every Reconfigure, and flagging those delta frames as keyframes
      // poisons receiver decode (a field log: key counter climbing
      // ~2/s, 11 PLIs, decode resets). With an infinite GOP NVENC never
      // volunteers a real IDR, so the forced flag is the whole truth.
      int32_t result = ProcessEncodedFrame(packet, input_frame, send_key_frame);
      if (result != WEBRTC_VIDEO_CODEC_OK) {
        return result;
      }
    }
  } catch (const NVENCException& e) {
    RTC_LOG(LS_ERROR) << "Failed EncodeFrame NvEncoder(AV1) " << e.what();
    return WEBRTC_VIDEO_CODEC_ENCODER_FAILURE;
  }

  return WEBRTC_VIDEO_CODEC_OK;
}

int32_t NvidiaAv1EncoderImpl::ProcessEncodedFrame(
    std::vector<uint8_t>& packet,
    const ::webrtc::VideoFrame& inputFrame,
    bool is_keyframe) {
  // TINHAT_AV1_TRACE=1: dump the head of each encoded frame. The AV1 RTP
  // packetizer rejects whole frames on any OBU parse error and its complaint
  // is RTC_DLOG (compiled out of release builds), so misframed NVENC output
  // fails with NO log at all — this probe is how we see what it was fed.
  if (getenv("TINHAT_AV1_TRACE")) {
    fprintf(stderr, "[nvenc-av1] frame %zu bytes key=%d head=",
            packet.size(), (int)is_keyframe);
    for (size_t i = 0; i < packet.size() && i < 32; i++)
      fprintf(stderr, "%02x", packet[i]);
    fprintf(stderr, "\n");
  }
  encoded_image_._encodedWidth = encoder_->GetEncodeWidth();
  encoded_image_._encodedHeight = encoder_->GetEncodeHeight();
  encoded_image_.SetRtpTimestamp(inputFrame.rtp_timestamp());
  encoded_image_.SetSimulcastIndex(0);
  encoded_image_.ntp_time_ms_ = inputFrame.ntp_time_ms();
  encoded_image_.capture_time_ms_ = inputFrame.render_time_ms();
  encoded_image_.rotation_ = inputFrame.rotation();
  encoded_image_.content_type_ = VideoContentType::UNSPECIFIED;
  encoded_image_.timing_.flags = VideoSendTiming::kInvalid;
  encoded_image_._frameType = is_keyframe ? VideoFrameType::kVideoFrameKey
                                          : VideoFrameType::kVideoFrameDelta;
  encoded_image_.SetColorSpace(inputFrame.color_space());
  encoded_image_.SetEncodedData(
      EncodedImageBuffer::Create(packet.data(), packet.size()));
  encoded_image_.set_size(packet.size());
  encoded_image_.qp_ = -1;

  // Dependency-descriptor bookkeeping, single L1T1 layer — the same contract
  // libwebrtc's libaom wrapper honors. The SFU and receivers use this for
  // frame-dependency tracking; keyframes also carry the template structure.
  auto layer_frames = svc_controller_->NextFrameConfig(is_keyframe);
  CodecSpecificInfo codec_specific;
  codec_specific.codecType = kVideoCodecAV1;
  codec_specific.end_of_picture = true;
  codec_specific.scalability_mode = ScalabilityMode::kL1T1;
  if (!layer_frames.empty()) {
    codec_specific.generic_frame_info =
        svc_controller_->OnEncodeDone(layer_frames[0]);
    if (is_keyframe && codec_specific.generic_frame_info) {
      codec_specific.template_structure =
          svc_controller_->DependencyStructure();
      codec_specific.template_structure->resolutions = {
          RenderResolution(encoded_image_._encodedWidth,
                           encoded_image_._encodedHeight)};
    }
  }

  const auto result =
      encoded_image_callback_->OnEncodedImage(encoded_image_, &codec_specific);
  if (result.error != EncodedImageCallback::Result::OK) {
    RTC_LOG(LS_ERROR) << "AV1 OnEncodedImage failed " << result.error;
    return WEBRTC_VIDEO_CODEC_ERROR;
  }
  return WEBRTC_VIDEO_CODEC_OK;
}

VideoEncoder::EncoderInfo NvidiaAv1EncoderImpl::GetEncoderInfo() const {
  EncoderInfo info;
  info.supports_native_handle = false;
  info.implementation_name = "NVIDIA AV1 Encoder";
  info.scaling_settings = VideoEncoder::ScalingSettings::kOff;
  info.is_hardware_accelerated = true;
  info.supports_simulcast = false;
  info.preferred_pixel_formats = {VideoFrameBuffer::Type::kI420};
  return info;
}

void NvidiaAv1EncoderImpl::SetRates(const RateControlParameters& parameters) {
  if (!encoder_) {
    RTC_LOG(LS_WARNING) << "SetRates() while uninitialized.";
    return;
  }
  if (parameters.framerate_fps < 1.0) {
    return;
  }
  if (parameters.bitrate.get_sum_bps() == 0) {
    configuration_.SetStreamState(false);
    return;
  }

  codec_.maxFramerate = static_cast<uint32_t>(parameters.framerate_fps);
  codec_.maxBitrate = parameters.bitrate.GetSpatialLayerSum(0);
  configuration_.target_bps = parameters.bitrate.GetSpatialLayerSum(0);
  configuration_.max_frame_rate = parameters.framerate_fps;
  if (configuration_.target_bps) {
    configuration_.SetStreamState(true);
  } else {
    configuration_.SetStreamState(false);
  }

  // The NVENC live-rate lesson, applied from birth: push rate changes into the LIVE session.
  const uint32_t applied_bps = nv_encode_config_.rcParams.averageBitRate;
  const uint32_t new_bps = configuration_.target_bps;
  const auto new_fps = static_cast<uint32_t>(parameters.framerate_fps);
  const bool bps_changed = new_bps > 0 &&
      (new_bps > applied_bps + applied_bps / 10 ||
       new_bps + new_bps / 10 < applied_bps);
  // >20% only: the allocator reports the MEASURED input rate, which jitters
  // +-1-3fps every second — reconfiguring on that meant a driver reconfigure
  // (and, on AV1, a re-emitted sequence header) every single second.
  const uint32_t applied_fps = nv_initialize_params_.frameRateNum;
  const bool fps_changed = new_fps > 0 && applied_fps > 0 &&
      (new_fps * 5 > applied_fps * 6 || new_fps * 6 < applied_fps * 5);
  if (bps_changed || fps_changed) {
    nv_initialize_params_.frameRateNum = new_fps;
    nv_initialize_params_.frameRateDen = 1;
    nv_encode_config_.rcParams.averageBitRate = new_bps;
    nv_encode_config_.rcParams.vbvBufferSize =
        (new_bps / (new_fps ? new_fps : 30)) * 5;
    nv_encode_config_.rcParams.vbvInitialDelay =
        nv_encode_config_.rcParams.vbvBufferSize;
    NV_ENC_RECONFIGURE_PARAMS rp = {};
    rp.version = NV_ENC_RECONFIGURE_PARAMS_VER;
    rp.reInitEncodeParams = nv_initialize_params_;
    rp.resetEncoder = 0;
    rp.forceIDR = 0;
    try {
      encoder_->Reconfigure(&rp);
      fprintf(stderr, "[nvenc-av1] SetRates -> reconfigured to %u kbps @ %u fps\n",
              new_bps / 1000, new_fps);
    } catch (const NVENCException& e) {
      fprintf(stderr, "[nvenc-av1] SetRates reconfigure FAILED: %s\n", e.what());
    }
  }
}

void NvidiaAv1EncoderImpl::LayerConfig::SetStreamState(bool send_stream) {
  if (send_stream && !sending) {
    key_frame_request = true;
  }
  sending = send_stream;
}

}  // namespace webrtc
