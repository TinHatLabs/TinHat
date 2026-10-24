/*
 * Copyright 2025 LiveKit, Inc.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

#include "livekit/video_decoder_factory.h"

#include <modules/video_coding/codecs/av1/av1_svc_config.h>
#include "api/environment/environment.h"
#include "api/video_codecs/av1_profile.h"
#include "api/video_codecs/sdp_video_format.h"
#include "livekit/objc_video_factory.h"
#include "media/base/media_constants.h"
#include "modules/video_coding/codecs/h264/include/h264.h"
#include "modules/video_coding/codecs/vp8/include/vp8.h"
#include "modules/video_coding/codecs/vp9/include/vp9.h"
#include "rtc_base/logging.h"

#if defined(RTC_DAV1D_IN_INTERNAL_DECODER_FACTORY)
#include "modules/video_coding/codecs/av1/dav1d_decoder.h"  // nogncheck
#endif

#ifdef WEBRTC_ANDROID
#include "livekit/android.h"
#endif

#if defined(USE_NVIDIA_VIDEO_CODEC)
#include "nvidia/nvidia_decoder_factory.h"
#endif

namespace livekit_ffi {

VideoDecoderFactory::VideoDecoderFactory() {
#ifdef __APPLE__
  factories_.push_back(livekit_ffi::CreateObjCVideoDecoderFactory());
#endif

#ifdef WEBRTC_ANDROID
  factories_.push_back(CreateAndroidVideoDecoderFactory());
#endif

#if defined(USE_NVIDIA_VIDEO_CODEC)
  // PATCH (Tin Hat): NVDEC H.264 RECEIVE is not robust to mid-stream resolution
  // changes (simulcast rung switches / layer upgrades): NvDecoder's reconfigure path
  // SEGVs on the DecodingQueue thread (dimension/pitch mismatch in the NV12->I420 copy
  // in h264_decoder_impl.cpp while queued frames still carry the old size) — reproduced
  // deterministically by the TINHAT_CAM_SIMTEST gate. Software H.264 decode (ffmpeg,
  // WEBRTC_USE_H264 — this factory's existing fallback) is cheap at <=1080p and handles
  // resolution changes correctly, so hardware DECODE is now OPT-IN via LK_NVDEC=1.
  // NVENC ENCODE is unaffected (that's the headline feature; separate factory).
  const char* nvdec = getenv("LK_NVDEC");
  if (nvdec && nvdec[0] == '1') {
    if (webrtc::NvidiaVideoDecoderFactory::IsSupported()) {
      factories_.push_back(std::make_unique<webrtc::NvidiaVideoDecoderFactory>());
    }
  } else {
    RTC_LOG(LS_INFO)
        << "NVDEC receive decode disabled (software decode; LK_NVDEC=1 opts back in)";
  }
#endif
}

std::vector<webrtc::SdpVideoFormat> VideoDecoderFactory::GetSupportedFormats()
    const {
  std::vector<webrtc::SdpVideoFormat> formats;

  for (const auto& factory : factories_) {
    auto supported_formats = factory->GetSupportedFormats();
    formats.insert(formats.end(), supported_formats.begin(),
                   supported_formats.end());
  }

  formats.push_back(webrtc::SdpVideoFormat(webrtc::kVp8CodecName));
  for (const webrtc::SdpVideoFormat& format :
       webrtc::SupportedVP9DecoderCodecs())
    formats.push_back(format);
  for (const webrtc::SdpVideoFormat& h264_format :
       webrtc::SupportedH264DecoderCodecs())
    formats.push_back(h264_format);

  formats.push_back(webrtc::SdpVideoFormat(
      webrtc::SdpVideoFormat::AV1Profile0(),
      webrtc::LibaomAv1EncoderSupportedScalabilityModes()));
  return formats;
}

VideoDecoderFactory::CodecSupport VideoDecoderFactory::QueryCodecSupport(
    const webrtc::SdpVideoFormat& format,
    bool reference_scaling) const {
  if (reference_scaling) {
    webrtc::VideoCodecType codec =
        webrtc::PayloadStringToCodecType(format.name);
    if (codec != webrtc::kVideoCodecVP9 && codec != webrtc::kVideoCodecAV1) {
      return {/*is_supported=*/false, /*is_power_efficient=*/false};
    }
  }

  CodecSupport codec_support;
  codec_support.is_supported = format.IsCodecInList(GetSupportedFormats());
  return codec_support;
}

std::unique_ptr<webrtc::VideoDecoder> VideoDecoderFactory::Create(
    const webrtc::Environment& env, const webrtc::SdpVideoFormat& format) {
  for (const auto& factory : factories_) {
    for (const auto& supported_format : factory->GetSupportedFormats()) {
      if (supported_format.IsSameCodec(format))
        return factory->Create(env, format);
    }
  }

  // IsSameCodec treats H.264 packetization-modes as distinct codecs, so when
  // the SFU sends mode=0 but the platform factory only advertises mode=1 the
  // strict match above fails. Retry with the factory's packetization-mode so
  // only that parameter is relaxed while the profile-level-id check is kept.
  if (absl::EqualsIgnoreCase(format.name, webrtc::kH264CodecName)) {
    for (const auto& factory : factories_) {
      for (const auto& sf : factory->GetSupportedFormats()) {
        if (!absl::EqualsIgnoreCase(sf.name, webrtc::kH264CodecName))
          continue;
        auto adjusted = format;
        auto it = sf.parameters.find("packetization-mode");
        if (it != sf.parameters.end())
          adjusted.parameters["packetization-mode"] = it->second;
        else
          adjusted.parameters.erase("packetization-mode");
        if (sf.IsSameCodec(adjusted))
          return factory->Create(env, adjusted);
      }
    }
  }

  if (absl::EqualsIgnoreCase(format.name, webrtc::kVp8CodecName))
    return webrtc::CreateVp8Decoder(env);
  if (absl::EqualsIgnoreCase(format.name, webrtc::kVp9CodecName))
    return webrtc::VP9Decoder::Create();
  if (absl::EqualsIgnoreCase(format.name, webrtc::kH264CodecName))
    return webrtc::H264Decoder::Create();


#if defined(RTC_DAV1D_IN_INTERNAL_DECODER_FACTORY)
  if (absl::EqualsIgnoreCase(format.name, webrtc::kAv1CodecName)) {
    return webrtc::CreateDav1dDecoder();
  }
#endif


  RTC_LOG(LS_ERROR) << "No VideoDecoder found for " << format.name;
  return nullptr;
}

}  // namespace livekit_ffi
