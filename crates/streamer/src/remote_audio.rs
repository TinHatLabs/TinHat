//! Pad Watch round 13 (launch-gate, share claude/tinhat-pad/WATCH-LEASE/ "ROUND 13"): the TYPE proof that the
//! remote-track taps (the per-track playback level `call::set_track_level`, the "can they hear me" receive tap
//! `hear::note_recv`) are fed only by RECEIVED tracks. They take a [`RemoteAudioFrame`], and the only way to get one is
//! [`RemoteAudioStream::next`], which only [`RemoteAudioStream::new`] makes, from a `livekit::track::RemoteAudioTrack`:
//! a type the SDK constructs only for a subscribed remote track (its `new` is `pub(crate)` in the livekit crate). A mic
//! frame, a sink-monitor (output mix) buffer or a local track's stream cannot be turned into one: the field is private
//! to this module and there is no other constructor. The compiler is the proof; the capture lint keeps
//! `RemoteAudioStream::new(` inside `call.rs::build_playback` as a regression guard.
use futures_util::StreamExt;
use livekit::webrtc::audio_frame::AudioFrame;
use livekit::webrtc::audio_stream::native::NativeAudioStream;

/// The decoded audio of ONE received remote track.
pub(crate) struct RemoteAudioStream(NativeAudioStream);

impl RemoteAudioStream {
    pub(crate) fn new(track: &livekit::track::RemoteAudioTrack, sample_rate: i32, channels: i32) -> Self {
        RemoteAudioStream(NativeAudioStream::new(track.rtc_track(), sample_rate, channels))
    }
    pub(crate) async fn next(&mut self) -> Option<RemoteAudioFrame> {
        self.0.next().await.map(RemoteAudioFrame)
    }
}

/// One decoded frame of a received remote track (read-only; constructible only by [`RemoteAudioStream::next`]).
pub(crate) struct RemoteAudioFrame(AudioFrame<'static>);

impl std::ops::Deref for RemoteAudioFrame {
    type Target = AudioFrame<'static>;
    fn deref(&self) -> &AudioFrame<'static> {
        &self.0
    }
}
