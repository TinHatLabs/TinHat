// PATCH (Tin Hat, NVENC AV1): NVENC AV1 encoder — the ultrawide/8K path.
// NVENC H.264 hard-caps at 4096x4096 (measured on Blackwell); AV1 encodes to
// 8192x8192 at roughly half the bitrate for the same quality. Patterned on
// h264_encoder_impl with the codec-specific parts swapped: no NALU/SPS-PPS
// machinery (AV1 is OBU-based, repeatSeqHdr puts the sequence header on every
// keyframe), and the dependency-descriptor info comes from a no-layering SVC
// controller like libwebrtc's own libaom wrapper provides.
#ifndef WEBRTC_NVIDIA_AV1_ENCODER_IMPL_H_
#define WEBRTC_NVIDIA_AV1_ENCODER_IMPL_H_

#include <cuda.h>

#include <memory>
#include <vector>

#include "NvEncoder/NvEncoder.h"
#include "NvEncoder/NvEncoderCuda.h"

#include "api/environment/environment.h"
#include "api/transport/rtp/dependency_descriptor.h"
#include "api/video/i420_buffer.h"
#include "api/video/video_codec_constants.h"
#include "api/video_codecs/scalability_mode.h"
#include "api/video_codecs/video_encoder.h"
#include "modules/video_coding/svc/scalable_video_controller.h"

namespace webrtc {

class NvidiaAv1EncoderImpl : public VideoEncoder {
 public:
  struct LayerConfig {
    bool sending = true;
    bool key_frame_request = false;
    float max_frame_rate = 0;
    uint32_t target_bps = 0;
    uint32_t max_bps = 0;

    void SetStreamState(bool send_stream);
  };

  NvidiaAv1EncoderImpl(const webrtc::Environment& env,
                       CUcontext context,
                       CUmemorytype memory_type,
                       NV_ENC_BUFFER_FORMAT nv_format,
                       const SdpVideoFormat& format);
  ~NvidiaAv1EncoderImpl() override;

  int32_t InitEncode(const VideoCodec* codec_settings,
                     const Settings& settings) override;
  int32_t RegisterEncodeCompleteCallback(
      EncodedImageCallback* callback) override;
  int32_t Release() override;
  int32_t Encode(const VideoFrame& frame,
                 const std::vector<VideoFrameType>* frame_types) override;
  void SetRates(const RateControlParameters& rc_parameters) override;
  EncoderInfo GetEncoderInfo() const override;

 private:
  int32_t ProcessEncodedFrame(std::vector<uint8_t>& packet,
                              const ::webrtc::VideoFrame& inputFrame,
                              bool is_keyframe);

  const webrtc::Environment& env_;
  EncodedImageCallback* encoded_image_callback_ = nullptr;

  std::unique_ptr<NvEncoder> encoder_;
  CUcontext cu_context_;
  CUmemorytype cu_memory_type_;
  NV_ENC_BUFFER_FORMAT nv_format_;
  NV_ENC_INITIALIZE_PARAMS nv_initialize_params_;
  NV_ENC_CONFIG nv_encode_config_;

  LayerConfig configuration_;
  EncodedImage encoded_image_;
  VideoCodec codec_;
  const SdpVideoFormat format_;
  // Emits the dependency-descriptor structures the RTP layer wants for AV1
  // (single layer, L1T1) — same contract as libwebrtc's libaom wrapper.
  std::unique_ptr<ScalableVideoController> svc_controller_;
};

}  // namespace webrtc

#endif  // WEBRTC_NVIDIA_AV1_ENCODER_IMPL_H_
