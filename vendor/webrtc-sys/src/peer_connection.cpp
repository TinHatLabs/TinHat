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

#include "livekit/peer_connection.h"
#include "livekit/peer_connection_factory.h"

#include <memory>
#include <sstream>
#include <cstring>

#include "api/data_channel_interface.h"
#include "api/peer_connection_interface.h"
#include "api/scoped_refptr.h"
#include "livekit/candidate.h"
#include "livekit/data_channel.h"
#include "livekit/jsep.h"
#include "livekit/media_stream.h"
#include "livekit/rtc_error.h"
#include "livekit/rtp_transceiver.h"
#include "rtc_base/logging.h"

namespace livekit_ffi {

webrtc::PeerConnectionInterface::RTCConfiguration to_native_rtc_configuration(
    RtcConfiguration config) {
  webrtc::PeerConnectionInterface::RTCConfiguration rtc_config{};

  for (auto item : config.ice_servers) {
    webrtc::PeerConnectionInterface::IceServer ice_server;
    ice_server.username = item.username.c_str();
    ice_server.password = item.password.c_str();

    for (auto url : item.urls)
      ice_server.urls.emplace_back(url.c_str());

    rtc_config.servers.push_back(ice_server);
  }

  rtc_config.continual_gathering_policy =
      static_cast<webrtc::PeerConnectionInterface::ContinualGatheringPolicy>(
          config.continual_gathering_policy);

  rtc_config.type =
      static_cast<webrtc::PeerConnectionInterface::IceTransportsType>(
          config.ice_transport_type);

  return rtc_config;
}

// PATCH (Tin Hat, BWE seed): seed the send-side bandwidth estimator by munging
// `x-google-start-bitrate` into the video codecs' fmtp of a session description.
//
// GCC starts near zero and, with a screen-share that always fills its allocation,
// never enters ALR — so it only additively climbs ~8-10%/s and every share spent
// its first minutes at ~6 Mbps regardless of preset, machine, or link.
//
// Findings that shaped this (all measured, 2026-08-12):
//   * PeerConnection::SetBitrate(start) returns ok and changes NOTHING on this
//     libwebrtc build — dead end.
//   * Munging the LOCAL description changes nothing either: for the SEND
//     direction, libwebrtc takes codec parameters from the REMOTE description
//     (fmtp expresses the receiver's preferences). So the REMOTE (the SFU's
//     answer) is what must carry the param — the same direction Jitsi has
//     munged for years. The SFU never sees the edit; we are only changing what
//     we feed our own stack.
//
// Env-driven (LK_BWE_START_KBPS, set around publish_track by the Rust side) so
// no cross-crate API plumbing is needed; unset = byte-identical no-op. rtx fmtp
// ("apt=") is left untouched.
static std::unique_ptr<webrtc::SessionDescriptionInterface> maybe_seed_start_bitrate(
    std::unique_ptr<webrtc::SessionDescriptionInterface> native,
    const char* which) {
  const char* seed = getenv("LK_BWE_START_KBPS");
  if (!seed) return native;
  int kbps = atoi(seed);
  if (kbps <= 0) return native;

  std::string sdp;
  native->ToString(&sdp);
  std::string out;
  out.reserve(sdp.size() + 512);
  std::istringstream lines(sdp);
  std::string line;
  bool in_video = false;
  int munged = 0;
  while (std::getline(lines, line)) {
    if (!line.empty() && line.back() == '\r') line.pop_back();
    if (line.rfind("m=", 0) == 0) in_video = line.rfind("m=video", 0) == 0;
    if (in_video && line.rfind("a=fmtp:", 0) == 0 &&
        line.find("apt=") == std::string::npos) {
      line += ";x-google-start-bitrate=" + std::to_string(kbps);
      munged++;
    }
    out += line;
    out += "\r\n";
  }
  if (munged == 0) return native;

  webrtc::SdpParseError err;
  auto replaced = webrtc::CreateSessionDescription(native->GetType(), out, &err);
  if (!replaced) {
    fprintf(stderr, "[bwe-seed] %s munge parse failed (%s) — using original SDP\n",
            which, err.description.c_str());
    return native;
  }
  fprintf(stderr, "[bwe-seed] %s: seeded x-google-start-bitrate=%d on %d video codec(s)\n",
          which, kbps, munged);
  return replaced;
}


// PATCH (Tin Hat, BWE seed), the decisive one: drop goog-remb from a remote
// description. With receiver_limit_caps_only_, the estimator's target is
// min()'d against the SFU's REMB, which ramps in lock-step with delivered
// traffic — a self-reinforcing ~8%/s crawl that NO local mechanism
// (SetBitrate, x-google-start-bitrate local/remote, allocation changes: all
// measured dead) can outrun. Removing goog-remb from the answer leaves TWCC
// send-side estimation in charge, which is the modern mechanism anyway.
// Gated on LK_STRIP_REMB=1 so default behavior is byte-identical.
static std::unique_ptr<webrtc::SessionDescriptionInterface> maybe_strip_remb(
    std::unique_ptr<webrtc::SessionDescriptionInterface> native) {
  const char* flag = getenv("LK_STRIP_REMB");
  if (!flag || strcmp(flag, "1") != 0) return native;

  std::string sdp;
  native->ToString(&sdp);
  std::string out;
  out.reserve(sdp.size());
  std::istringstream lines(sdp);
  std::string line;
  int stripped = 0;
  while (std::getline(lines, line)) {
    if (!line.empty() && line.back() == '\r') line.pop_back();
    if (line.rfind("a=rtcp-fb:", 0) == 0 &&
        line.find("goog-remb") != std::string::npos) {
      stripped++;
      continue;
    }
    out += line;
    out += "\r\n";
  }
  if (stripped == 0) return native;

  webrtc::SdpParseError err;
  auto replaced = webrtc::CreateSessionDescription(native->GetType(), out, &err);
  if (!replaced) {
    fprintf(stderr, "[bwe-seed] remb-strip parse failed (%s) — using original SDP\n",
            err.description.c_str());
    return native;
  }
  fprintf(stderr, "[bwe-seed] stripped %d goog-remb line(s) from the remote answer\n",
          stripped);
  return replaced;
}

inline webrtc::PeerConnectionInterface::RTCOfferAnswerOptions
to_native_offer_answer_options(const RtcOfferAnswerOptions& options) {
  webrtc::PeerConnectionInterface::RTCOfferAnswerOptions rtc_options;
  rtc_options.offer_to_receive_video = options.offer_to_receive_video;
  rtc_options.offer_to_receive_audio = options.offer_to_receive_audio;
  rtc_options.voice_activity_detection = options.voice_activity_detection;
  rtc_options.ice_restart = options.ice_restart;
  rtc_options.use_rtp_mux = options.use_rtp_mux;
  rtc_options.raw_packetization_for_video = options.raw_packetization_for_video;
  rtc_options.num_simulcast_layers = options.num_simulcast_layers;
  rtc_options.use_obsolete_sctp_sdp = options.use_obsolete_sctp_sdp;
  return rtc_options;
}

PeerConnection::PeerConnection(
    std::shared_ptr<RtcRuntime> rtc_runtime,
    webrtc::scoped_refptr<webrtc::PeerConnectionFactoryInterface> pc_factory,
    rust::Box<PeerConnectionObserverWrapper> observer)
    : rtc_runtime_(std::move(rtc_runtime)),
      pc_factory_(std::move(pc_factory)),
      observer_(std::move(observer)) {
  RTC_LOG(LS_VERBOSE) << "PeerConnection::PeerConnection()";
}

PeerConnection::~PeerConnection() {
  RTC_LOG(LS_VERBOSE) << "PeerConnection::~PeerConnection()";
}

bool PeerConnection::Initialize(
    webrtc::PeerConnectionInterface::RTCConfiguration config) {
  webrtc::PeerConnectionDependencies deps{this};
  auto result =
      pc_factory_->CreatePeerConnectionOrError(config, std::move(deps));

  if (!result.ok()) {
    RTC_LOG(LS_ERROR) << "Failed to create peer connection: "
                      << result.error().message();
    return false;
  }
  peer_connection_ = std::move(result.value());
  return true;
}

void PeerConnection::set_configuration(RtcConfiguration config) const {
  auto result =
      peer_connection_->SetConfiguration(to_native_rtc_configuration(config));

  if (!result.ok()) {
    throw std::runtime_error(serialize_error(to_error(result)));
  }
}

void PeerConnection::create_offer(
    RtcOfferAnswerOptions options,
    rust::Box<PeerContext> ctx,
    rust::Fn<void(rust::Box<PeerContext>, std::unique_ptr<SessionDescription>)>
        on_success,
    rust::Fn<void(rust::Box<PeerContext>, RtcError)> on_error) const {
  webrtc::scoped_refptr<NativeCreateSdpObserver> observer =
      webrtc::make_ref_counted<NativeCreateSdpObserver>(std::move(ctx), on_success,
                                                     on_error);

  peer_connection_->CreateOffer(observer.get(),
                                to_native_offer_answer_options(options));
}

void PeerConnection::create_answer(
    RtcOfferAnswerOptions options,
    rust::Box<PeerContext> ctx,
    rust::Fn<void(rust::Box<PeerContext>, std::unique_ptr<SessionDescription>)>
        on_success,
    rust::Fn<void(rust::Box<PeerContext>, RtcError)> on_error) const {
  webrtc::scoped_refptr<NativeCreateSdpObserver> observer =
      webrtc::make_ref_counted<NativeCreateSdpObserver>(std::move(ctx), on_success,
                                                     on_error);

  peer_connection_->CreateAnswer(observer.get(),
                                 to_native_offer_answer_options(options));
}

void PeerConnection::set_local_description(
    std::unique_ptr<SessionDescription> desc,
    rust::Box<PeerContext> ctx,
    rust::Fn<void(rust::Box<PeerContext>, RtcError)> on_complete) const {
  webrtc::scoped_refptr<NativeSetLocalSdpObserver> observer =
      webrtc::make_ref_counted<NativeSetLocalSdpObserver>(std::move(ctx),
                                                       on_complete);

  // PATCH (Tin Hat, BWE seed): see maybe_seed_start_bitrate above.
  //
  // ALSO register the start constraint via SetBitrate BEFORE this SLD. On an
  // already-running estimator this is a measured no-op (clamped to the current
  // delay-based limit) — but when the env var is set BEFORE Room::connect, this
  // fires on the very FIRST negotiation, the constraint is in place when the
  // estimator initializes, and the initial probe sequence launches from the
  // seed (3x/6x) instead of from the 300 kbps default. That is the only moment
  // this build honors a start rate — every later mechanism (SetBitrate,
  // local/remote munge, allocation staircase) was tried and measured dead.
  if (const char* seed = getenv("LK_BWE_START_KBPS")) {
    int kbps = atoi(seed);
    if (kbps > 0) {
      webrtc::BitrateSettings bs;
      // All three fields, not just start: a lone start did not survive the
      // bitrate configurator's merge (measured — the probe controller kept
      // firing its initial clusters at the default-derived ~5 Mbps, and a
      // cluster's measurement replaces the estimate, so the seed was erased
      // within 200 ms every time).
      bs.min_bitrate_bps = 30'000;
      bs.start_bitrate_bps = kbps * 1000;
      bs.max_bitrate_bps = 200'000'000;
      auto err = peer_connection_->SetBitrate(bs);
      fprintf(stderr, "[bwe-seed] SLD SetBitrate(min/start/max, start=%d kbps) -> %s\n",
              kbps, err.ok() ? "ok" : err.message());
    }
  }
  peer_connection_->SetLocalDescription(
      maybe_seed_start_bitrate(desc->clone()->release(), "SLD"), observer);
}

void PeerConnection::set_remote_description(
    std::unique_ptr<SessionDescription> desc,
    rust::Box<PeerContext> ctx,
    rust::Fn<void(rust::Box<PeerContext>, RtcError)> on_complete) const {
  webrtc::scoped_refptr<NativeSetRemoteSdpObserver> observer =
      webrtc::make_ref_counted<NativeSetRemoteSdpObserver>(std::move(ctx),
                                                        on_complete);

  // PATCH (Tin Hat, BWE seed): both remote-side munges live here — send-side
  // codec parameters come from the remote description (the SFU's answer), and
  // maybe_strip_remb (see its comment) is the one that kills the REMB cap.
  peer_connection_->SetRemoteDescription(
      maybe_strip_remb(
          maybe_seed_start_bitrate(desc->clone()->release(), "SRD")),
      observer);
}

void PeerConnection::restart_ice() const {
  peer_connection_->RestartIce();
}

void PeerConnection::add_ice_candidate(
    std::shared_ptr<IceCandidate> candidate,
    rust::Box<PeerContext> ctx,
    rust::Fn<void(rust::Box<PeerContext>, RtcError)> on_complete) const {
  peer_connection_->AddIceCandidate(
      candidate->release(), [&](const webrtc::RTCError& err) {
        on_complete(std::move(ctx), to_error(err));
      });
}

std::shared_ptr<DataChannel> PeerConnection::create_data_channel(
    rust::String label,
    DataChannelInit init) const {
  webrtc::DataChannelInit rtc_init = to_native_data_channel_init(init);
  auto result =
      peer_connection_->CreateDataChannelOrError(label.c_str(), &rtc_init);

  if (!result.ok()) {
    throw std::runtime_error(serialize_error(to_error(result.error())));
  }

  return std::make_shared<DataChannel>(rtc_runtime_, result.value());
}

std::shared_ptr<RtpSender> PeerConnection::add_track(
    std::shared_ptr<MediaStreamTrack> track,
    const rust::Vec<rust::String>& stream_ids) const {
  std::vector<std::string> std_stream_ids(stream_ids.begin(), stream_ids.end());
  auto result = peer_connection_->AddTrack(track->rtc_track(), std_stream_ids);
  if (!result.ok()) {
    throw std::runtime_error(serialize_error(to_error(result.error())));
  }

  return std::make_shared<RtpSender>(rtc_runtime_, result.value(),
                                     peer_connection_);
}

void PeerConnection::remove_track(std::shared_ptr<RtpSender> sender) const {
  auto error = peer_connection_->RemoveTrackOrError(sender->rtc_sender());
  if (!error.ok())
    throw std::runtime_error(serialize_error(to_error(error)));
}

void PeerConnection::get_stats(
    rust::Box<PeerContext> ctx,
    rust::Fn<void(rust::Box<PeerContext>, rust::String)> on_stats) const {
  auto observer = webrtc::make_ref_counted<NativeRtcStatsCollector<PeerContext>>(
      std::move(ctx), on_stats);
  peer_connection_->GetStats(observer.get());
}

std::shared_ptr<RtpTransceiver> PeerConnection::add_transceiver(
    std::shared_ptr<MediaStreamTrack> track,
    RtpTransceiverInit init) const {
  auto result = peer_connection_->AddTransceiver(
      track->rtc_track(), to_native_rtp_transceiver_init(init));
  if (!result.ok())
    throw std::runtime_error(serialize_error(to_error(result.error())));

  return std::make_shared<RtpTransceiver>(rtc_runtime_, result.value(),
                                          peer_connection_);
}

std::shared_ptr<RtpTransceiver> PeerConnection::add_transceiver_for_media(
    MediaType media_type,
    RtpTransceiverInit init) const {
  auto result = peer_connection_->AddTransceiver(
      static_cast<webrtc::MediaType>(media_type),
      to_native_rtp_transceiver_init(init));

  if (!result.ok())
    throw std::runtime_error(serialize_error(to_error(result.error())));

  return std::make_shared<RtpTransceiver>(rtc_runtime_, result.value(),
                                          peer_connection_);
}

rust::Vec<RtpSenderPtr> PeerConnection::get_senders() const {
  rust::Vec<RtpSenderPtr> vec;
  for (auto sender : peer_connection_->GetSenders())
    vec.push_back(RtpSenderPtr{
        std::make_shared<RtpSender>(rtc_runtime_, sender, peer_connection_)});

  return vec;
}

rust::Vec<RtpReceiverPtr> PeerConnection::get_receivers() const {
  rust::Vec<RtpReceiverPtr> vec;
  for (auto receiver : peer_connection_->GetReceivers())
    vec.push_back(RtpReceiverPtr{std::make_shared<RtpReceiver>(
        rtc_runtime_, receiver, peer_connection_)});

  return vec;
}

rust::Vec<RtpTransceiverPtr> PeerConnection::get_transceivers() const {
  rust::Vec<RtpTransceiverPtr> vec;
  for (auto transceiver : peer_connection_->GetTransceivers())
    vec.push_back(RtpTransceiverPtr{std::make_shared<RtpTransceiver>(
        rtc_runtime_, transceiver, peer_connection_)});

  return vec;
}

std::unique_ptr<SessionDescription> PeerConnection::current_local_description()
    const {
  auto local_description = peer_connection_->current_local_description();
  if (local_description)
    return std::make_unique<SessionDescription>(local_description->Clone());

  return nullptr;
}

std::unique_ptr<SessionDescription> PeerConnection::current_remote_description()
    const {
  auto remote_description = peer_connection_->current_remote_description();
  if (remote_description)
    return std::make_unique<SessionDescription>(remote_description->Clone());

  return nullptr;
}

std::unique_ptr<SessionDescription> PeerConnection::pending_local_description()
    const {
  auto local_description = peer_connection_->pending_local_description();
  if (local_description)
    return std::make_unique<SessionDescription>(local_description->Clone());

  return nullptr;
}

std::unique_ptr<SessionDescription> PeerConnection::pending_remote_description()
    const {
  auto remote_description = peer_connection_->pending_remote_description();
  if (remote_description)
    return std::make_unique<SessionDescription>(remote_description->Clone());

  return nullptr;
}

std::unique_ptr<SessionDescription> PeerConnection::local_description() const {
  auto local_description = peer_connection_->local_description();
  if (local_description)
    return std::make_unique<SessionDescription>(local_description->Clone());

  return nullptr;
}

std::unique_ptr<SessionDescription> PeerConnection::remote_description() const {
  auto remote_description = peer_connection_->remote_description();
  if (remote_description)
    return std::make_unique<SessionDescription>(remote_description->Clone());

  return nullptr;
}

PeerConnectionState PeerConnection::connection_state() const {
  return static_cast<PeerConnectionState>(
      peer_connection_->peer_connection_state());
}

SignalingState PeerConnection::signaling_state() const {
  return static_cast<SignalingState>(peer_connection_->signaling_state());
}

IceGatheringState PeerConnection::ice_gathering_state() const {
  return static_cast<IceGatheringState>(
      peer_connection_->ice_gathering_state());
}

IceConnectionState PeerConnection::ice_connection_state() const {
  return static_cast<IceConnectionState>(
      peer_connection_->ice_connection_state());
}

void PeerConnection::close() const {
  peer_connection_->Close();
}

// PeerConnectionObserver

void PeerConnection::OnSignalingChange(
    webrtc::PeerConnectionInterface::SignalingState new_state) {
  observer_->on_signaling_change(static_cast<SignalingState>(new_state));
}

void PeerConnection::OnAddStream(
    webrtc::scoped_refptr<webrtc::MediaStreamInterface> stream) {
  observer_->on_add_stream(std::make_unique<MediaStream>(rtc_runtime_, stream));
}

void PeerConnection::OnRemoveStream(
    webrtc::scoped_refptr<webrtc::MediaStreamInterface> stream) {
  // Find current MediaStream
  // observer_->on_remove_stream(std::make_unique<MediaStream>(rtc_runtime_,
  // stream));
}

void PeerConnection::OnDataChannel(
    webrtc::scoped_refptr<webrtc::DataChannelInterface> data_channel) {
  observer_->on_data_channel(
      std::make_shared<DataChannel>(rtc_runtime_, data_channel));
}

void PeerConnection::OnRenegotiationNeeded() {
  observer_->on_renegotiation_needed();
}

void PeerConnection::OnNegotiationNeededEvent(uint32_t event_id) {
  observer_->on_negotiation_needed_event(event_id);
}

void PeerConnection::OnIceConnectionChange(
    webrtc::PeerConnectionInterface::IceConnectionState new_state) {
  observer_->on_ice_connection_change(
      static_cast<IceConnectionState>(new_state));
}

void PeerConnection::OnStandardizedIceConnectionChange(
    webrtc::PeerConnectionInterface::IceConnectionState new_state) {
  observer_->on_standardized_ice_connection_change(
      static_cast<IceConnectionState>(new_state));
}

void PeerConnection::OnConnectionChange(
    webrtc::PeerConnectionInterface::PeerConnectionState new_state) {
  // PATCH (Tin Hat, BWE seed), second half: re-apply the estimator seed the
  // moment the transport actually comes up. The SetLocalDescription hook alone
  // is not enough for a FRESH peer connection (the probe's scratch room, or a
  // share published straight after joining): GCC's network-available event
  // fires AFTER the SLD and resets the estimate to its default start rate,
  // silently discarding the seed (measured: t=1 grant ~4.8 Mbps with a 15 Mbps
  // seed applied at SLD). On an already-connected PC (the common in-call share)
  // this callback doesn't re-fire and the SLD hook is the one that matters.
  if (new_state ==
      webrtc::PeerConnectionInterface::PeerConnectionState::kConnected) {
    if (const char* seed = getenv("LK_BWE_START_KBPS")) {
      int kbps = atoi(seed);
      if (kbps > 0) {
        webrtc::BitrateSettings bs;
        bs.start_bitrate_bps = kbps * 1000;
        auto err = peer_connection_->SetBitrate(bs);
        fprintf(stderr, "[bwe-seed] connect seed %d kbps -> %s\n", kbps,
                err.ok() ? "ok" : err.message());
      }
    }
  }
  observer_->on_connection_change(static_cast<PeerConnectionState>(new_state));
}

void PeerConnection::OnIceGatheringChange(
    webrtc::PeerConnectionInterface::IceGatheringState new_state) {
  observer_->on_ice_gathering_change(static_cast<IceGatheringState>(new_state));
}

void PeerConnection::OnIceCandidate(
    const webrtc::IceCandidate* candidate) {
  auto new_candidate = webrtc::CreateIceCandidate(candidate->sdp_mid(),
                                                  candidate->sdp_mline_index(),
                                                  candidate->candidate());
  observer_->on_ice_candidate(
      std::make_unique<IceCandidate>(std::move(new_candidate)));
}

void PeerConnection::OnIceCandidateError(const std::string& address,
                                         int port,
                                         const std::string& url,
                                         int error_code,
                                         const std::string& error_text) {
  observer_->on_ice_candidate_error(address, port, url, error_code, error_text);
}

void PeerConnection::OnIceCandidateRemoved(const webrtc::IceCandidate* ice_candidate) {
  rust::Vec<CandidatePtr> vec;
  if(ice_candidate != nullptr) {
    vec.push_back(CandidatePtr{std::make_unique<Candidate>(ice_candidate->candidate())});
  }
  observer_->on_ice_candidates_removed(std::move(vec));
}

void PeerConnection::OnIceConnectionReceivingChange(bool receiving) {
  observer_->on_ice_connection_receiving_change(receiving);
}

void PeerConnection::OnIceSelectedCandidatePairChanged(
    const webrtc::CandidatePairChangeEvent& event) {
  CandidatePairChangeEvent e{};
  e.selected_candidate_pair.local =
      std::make_unique<Candidate>(event.selected_candidate_pair.local);
  e.selected_candidate_pair.remote =
      std::make_unique<Candidate>(event.selected_candidate_pair.remote);
  e.last_data_received_ms = event.last_data_received_ms;
  e.reason = event.reason;
  e.estimated_disconnected_time_ms = event.estimated_disconnected_time_ms;

  observer_->on_ice_selected_candidate_pair_changed(std::move(e));
}

void PeerConnection::OnAddTrack(
    webrtc::scoped_refptr<webrtc::RtpReceiverInterface> receiver,
    const std::vector<webrtc::scoped_refptr<webrtc::MediaStreamInterface>>&
        streams) {
  rust::Vec<MediaStreamPtr> vec;

  for (const auto& item : streams) {
    vec.push_back(
        MediaStreamPtr{std::make_unique<MediaStream>(rtc_runtime_, item)});
  }

  observer_->on_add_track(
      std::make_unique<RtpReceiver>(rtc_runtime_, receiver, peer_connection_),
      std::move(vec));
}

void PeerConnection::OnTrack(
    webrtc::scoped_refptr<webrtc::RtpTransceiverInterface> transceiver) {
  observer_->on_track(std::make_unique<RtpTransceiver>(
      rtc_runtime_, transceiver, peer_connection_));
}

void PeerConnection::OnRemoveTrack(
    webrtc::scoped_refptr<webrtc::RtpReceiverInterface> receiver) {
  observer_->on_remove_track(
      std::make_unique<RtpReceiver>(rtc_runtime_, receiver, peer_connection_));
}

void PeerConnection::OnInterestingUsage(int usage_pattern) {
  observer_->on_interesting_usage(usage_pattern);
}

}  // namespace livekit_ffi
