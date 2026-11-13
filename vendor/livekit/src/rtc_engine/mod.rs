// Copyright 2025 LiveKit, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use libwebrtc::prelude::*;
use livekit_api::signal_client::{SignalError, SignalOptions};
use livekit_datatrack::backend as dt;
use livekit_protocol as proto;
use livekit_runtime::{interval, Interval, JoinHandle, MissedTickBehavior};
use parking_lot::{RwLock, RwLockReadGuard};
use std::{borrow::Cow, fmt::Debug, sync::Arc, time::Duration};
use thiserror::Error;
use tokio::sync::{
    mpsc, oneshot, Mutex as AsyncMutex, Notify, RwLock as AsyncRwLock,
    RwLockReadGuard as AsyncRwLockReadGuard,
};

pub use self::rtc_session::{SessionStats, INITIAL_BUFFERED_AMOUNT_LOW_THRESHOLD};
pub(crate) use self::peer_transport::SendGateGuard;
/// TINHAT PATCH (12.1.5): the offer side of the send gate, public for the app's unit tests only.
#[doc(hidden)]
pub use self::peer_transport::{take_offer_gate, SEND_GATE_WAIT};
use crate::prelude::ParticipantIdentity;
use crate::{
    id::ParticipantSid,
    options::TrackPublishOptions,
    prelude::LocalTrack,
    room::DisconnectReason,
    rtc_engine::{
        lk_runtime::LkRuntime,
        rtc_session::{RtcSession, SessionEvent, SessionEvents},
    },
    DataPacketKind,
};
use crate::{ChatMessage, E2eeManager, TranscriptionSegment};

mod dc_sender;
pub mod lk_runtime;
mod peer_transport;
mod rtc_events;
mod rtc_session;

pub(crate) type EngineEmitter = mpsc::UnboundedSender<EngineEvent>;
pub(crate) type EngineEvents = mpsc::UnboundedReceiver<EngineEvent>;
pub(crate) type EngineResult<T> = Result<T, EngineError>;

pub const RECONNECT_ATTEMPTS: u32 = 10;
pub const RECONNECT_INTERVAL: Duration = Duration::from_secs(5);

// TINHAT PATCH (reconnect-wedge, 12.1.13): no step of a reconnect may wait forever. A resume over a
// black-holed link (Wi-Fi freeze, SFU unreachable) used to sit in its first attempt for 14+ minutes:
// closing the old signal websocket waits for its read task, which never ends until TCP gives up. The
// reconnect holds `reconnecting_lock` the whole time and every add_track / create_sender waits on that
// lock, so a screen share pressed meanwhile hung too, with no error. Each attempt is now bounded; a
// timed-out resume escalates to a full reconnect like any failed resume, a timed-out restart counts as a
// failed attempt. The bounds sit above the SDK's own inner timeouts (signal connect 5 s + reconnect
// response 5 s + ICE 15 s), so a slow but live link still resumes.
pub const RESUME_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(30);
pub const RESTART_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(35);
/// Closing a session (signal + PCs) on the way to a restart or on close(): bounded, the old link may be dead. Above the
/// session's own three CLOSE_STEP_TIMEOUT steps, so its transports are always closed before this cuts in.
pub const SESSION_CLOSE_TIMEOUT: Duration = Duration::from_secs(8);
/// The room task's answer to Resuming / Restarting / SignalResumed / SignalRestarted / Resumed / Restarted.
pub const ROOM_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// TINHAT PATCH (reconnect-wedge, Astra r1 P1): a session a restart attempt made that never became the engine's. Dropped
/// uncommitted (the attempt failed, timed out or was cancelled by close()) it is closed, bounded, so it can't stay on the
/// SFU under our identity (a ghost the next join would collide with: DUPLICATE_IDENTITY). Upstream leaked it on every
/// failed wait_pc_connection too.
struct ProvisionalSession(Option<RtcSession>);

impl ProvisionalSession {
    fn get(&self) -> &RtcSession {
        self.0.as_ref().expect("provisional session present until committed")
    }

    fn commit(mut self) -> RtcSession {
        self.0.take().expect("provisional session committed once")
    }
}

impl Drop for ProvisionalSession {
    fn drop(&mut self) {
        if let Some(session) = self.0.take() {
            log::warn!("closing the session of a restart attempt that did not complete");
            livekit_runtime::spawn(async move {
                bounded(
                    SESSION_CLOSE_TIMEOUT,
                    "closing an unused restart session",
                    session.close(DisconnectReason::ClientInitiated),
                )
                .await;
            });
        }
    }
}

/// TINHAT PATCH (reconnect-wedge): await `fut` for at most `limit`; None = it timed out (logged with `what`).
async fn bounded<F: std::future::Future>(limit: Duration, what: &str, fut: F) -> Option<F::Output> {
    match tokio::time::timeout(limit, fut).await {
        Ok(v) => Some(v),
        Err(_) => {
            log::error!("{} took longer than {:?}; going on without it", what, limit);
            None
        }
    }
}

/// Settling delay before checking PeerConnection state on the resume path.
///
/// Lets a freshly issued ICE-restart offer/answer round-trip take effect when the
/// underlying PC was still in `Connected` at the moment we started the reconnect
/// (e.g. signal-only failure). Without this, the resume can return success
/// immediately and the next failure detector then trips the engine into a real
/// disconnect.
///
/// Only applied to the resume path. Full reconnect builds brand-new PCs which
/// don't suffer from the "looks-Connected-but-isn't" race.
pub const PC_RECONNECT_SETTLE_DELAY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SimulateScenario {
    /// Closes the signal channel locally; the engine attempts a Resume.
    SignalReconnect,
    Speaker,
    NodeFailure,
    ServerLeave,
    Migration,
    ForceTcp,
    ForceTls,
    /// Tells the server to issue a `LeaveRequest{Reconnect}`, forcing a
    /// full reconnect (new RtcSession, republish required).
    FullReconnect,
}

#[derive(Error, Debug)]
pub enum EngineError {
    #[error("signal failure: {0}")]
    Signal(#[from] SignalError),
    #[error("internal webrtc failure")]
    Rtc(#[from] RtcError),
    #[error("connection error: {0}")]
    Connection(Cow<'static, str>), // Connectivity issues (Failed to connect/reconnect)
    #[error("internal error: {0}")]
    Internal(Cow<'static, str>), // Unexpected error, generally we can't recover
}

#[derive(Default, Debug, Clone)]
pub struct EngineOptions {
    pub rtc_config: RtcConfiguration,
    pub signal_options: SignalOptions,
    pub join_retries: u32,
    /// Enable single peer connection mode
    pub single_peer_connection: bool,
}

#[derive(Debug)]
pub enum EngineEvent {
    ParticipantUpdate {
        updates: Vec<proto::ParticipantInfo>,
    },
    MediaTrack {
        track: MediaStreamTrack,
        stream: MediaStream,
        transceiver: RtpTransceiver,
    },
    Data {
        participant_sid: Option<ParticipantSid>,
        participant_identity: Option<ParticipantIdentity>,
        payload: Vec<u8>,
        topic: Option<String>,
        kind: DataPacketKind,
        encryption_type: proto::encryption::Type,
    },
    ChatMessage {
        participant_identity: ParticipantIdentity,
        message: ChatMessage,
    },
    Transcription {
        participant_identity: ParticipantIdentity,
        track_sid: String,
        segments: Vec<TranscriptionSegment>,
    },
    SipDTMF {
        participant_identity: Option<ParticipantIdentity>,
        code: u32,
        digit: Option<String>,
    },
    RpcRequest {
        caller_identity: Option<ParticipantIdentity>,
        request_id: String,
        method: String,
        payload: String,
        response_timeout: Duration,
        version: u32,
    },
    RpcResponse {
        request_id: String,
        payload: Option<String>,
        error: Option<proto::RpcError>,
    },
    RpcAck {
        request_id: String,
    },
    SpeakersChanged {
        speakers: Vec<proto::SpeakerInfo>,
    },
    ConnectionQuality {
        updates: Vec<proto::ConnectionQualityInfo>,
    },
    RoomUpdate {
        room: proto::Room,
    },
    RoomMoved {
        moved: proto::RoomMovedResponse,
    },
    /// The following events are used to notify the room about the reconnection state
    /// Since the room needs to also sync state in a good timing with the server.
    /// We synchronize the state with a one-shot channel.
    Resuming(oneshot::Sender<()>),
    Resumed(oneshot::Sender<()>),
    SignalResumed {
        reconnect_response: proto::ReconnectResponse,
        tx: oneshot::Sender<()>,
    },
    Restarting(oneshot::Sender<()>),
    Restarted(oneshot::Sender<()>),
    SignalRestarted {
        join_response: proto::JoinResponse,
        tx: oneshot::Sender<()>,
    },
    Disconnected {
        reason: DisconnectReason,
    },
    LocalTrackSubscribed {
        track_sid: String,
    },
    DataStreamHeader {
        header: proto::data_stream::Header,
        participant_identity: String,
        encryption_type: proto::encryption::Type,
    },
    DataStreamChunk {
        chunk: proto::data_stream::Chunk,
        participant_identity: String,
        encryption_type: proto::encryption::Type,
    },
    DataStreamTrailer {
        trailer: proto::data_stream::Trailer,
        participant_identity: String,
    },
    DataChannelBufferedAmountLowThresholdChanged {
        kind: DataPacketKind,
        threshold: u64,
    },
    RefreshToken {
        url: String,
        token: String,
    },
    TrackMuted {
        sid: String,
        muted: bool,
    },
    SubscribedQualityUpdate {
        update: proto::SubscribedQualityUpdate,
    },
    LocalDataTrackInput(dt::local::InputEvent),
    RemoteDataTrackInput(dt::remote::InputEvent),
}

/// Represents a running RtcSession with the ability to close the session
/// and the engine_task
#[derive(Debug)]
struct EngineHandle {
    session: Arc<RtcSession>,
    closed: bool,
    reconnecting: bool,
    can_reconnect: bool,

    // If full_reconnect is true, the next attempt will not try to resume
    // and will instead do a full reconnect
    full_reconnect: bool,
    engine_task: Option<(JoinHandle<()>, oneshot::Sender<()>)>,
}

struct EngineInner {
    // Keep a strong reference to LkRuntime to avoid creating a new RtcRuntime or PeerConnection
    // factory accross multiple Rtc sessions
    #[allow(dead_code)]
    lk_runtime: Arc<LkRuntime>,
    engine_tx: EngineEmitter,
    options: EngineOptions,

    close_notifier: Arc<Notify>,
    running_handle: RwLock<EngineHandle>,

    // The lock is write guarded for the whole reconnection time.
    // We can simply wait for reconnection by trying to acquire a read lock.
    // (This also prevents new reconnection to happens if a read guard is still held)
    reconnecting_lock: AsyncRwLock<()>,
    reconnecting_interval: AsyncMutex<Interval>,
}

pub struct RtcEngine {
    inner: Arc<EngineInner>,
}

impl Debug for RtcEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RtcEngine").finish()
    }
}

impl RtcEngine {
    pub async fn connect(
        url: &str,
        token: &str,
        options: EngineOptions,
        e2ee_manager: Option<E2eeManager>,
    ) -> EngineResult<(Self, proto::JoinResponse, EngineEvents)> {
        let (inner, join_response, engine_events) =
            EngineInner::connect(url, token, options, e2ee_manager).await?;
        Ok((Self { inner }, join_response, engine_events))
    }

    pub async fn close(&self, reason: DisconnectReason) {
        self.inner.close(reason).await
    }

    pub async fn publish_data(
        &self,
        data: proto::DataPacket,
        kind: DataPacketKind,
        is_raw_packet: bool,
    ) -> EngineResult<()> {
        let (session, _r_lock) = {
            let (handle, _r_lock) = self.inner.wait_reconnection().await?;
            (handle.session.clone(), _r_lock)
        };
        session.publish_data(data, kind, is_raw_packet).await
    }

    pub async fn simulate_scenario(&self, scenario: SimulateScenario) -> EngineResult<()> {
        let (session, _r_lock) = {
            let (handle, _r_lock) = self.inner.wait_reconnection().await?;
            (handle.session.clone(), _r_lock)
        };
        session.simulate_scenario(scenario).await
    }

    pub async fn handle_local_data_track_output(
        &self,
        event: dt::local::OutputEvent,
    ) -> EngineResult<()> {
        let (session, _r_lock) = {
            let (handle, _r_lock) = self.inner.wait_reconnection().await?;
            (handle.session.clone(), _r_lock)
        };
        session.handle_local_data_track_output(event).await;
        Ok(())
    }

    pub async fn handle_remote_data_track_output(
        &self,
        event: dt::remote::OutputEvent,
    ) -> EngineResult<()> {
        let (session, _r_lock) = {
            let (handle, _r_lock) = self.inner.wait_reconnection().await?;
            (handle.session.clone(), _r_lock)
        };
        session.handle_remote_data_track_output(event).await;
        Ok(())
    }

    pub async fn add_track(&self, req: proto::AddTrackRequest) -> EngineResult<proto::TrackInfo> {
        let (session, _r_lock) = {
            let (handle, _r_lock) = self.inner.wait_reconnection().await?;
            (handle.session.clone(), _r_lock)
        };
        session.add_track(req).await
    }

    pub fn remove_track(&self, sender: RtpSender) -> EngineResult<()> {
        // We don't need to wait for the reconnection
        let session = self.inner.running_handle.read().session.clone();
        session.remove_track(sender) // TODO(theomonnom): Ignore errors where this
                                     // RtpSender is bound to the old session. (Can
                                     // happen on bad timing and it is safe to ignore)
    }

    pub async fn mute_track(&self, req: proto::MuteTrackRequest) -> EngineResult<()> {
        let (session, _r_lock) = {
            let (handle, _r_lock) = self.inner.wait_reconnection().await?;
            (handle.session.clone(), _r_lock)
        };
        session.mute_track(req).await
    }

    pub async fn create_sender(
        &self,
        track: LocalTrack,
        options: TrackPublishOptions,
        encodings: Vec<RtpEncodingParameters>,
    ) -> EngineResult<(RtpTransceiver, SendGateGuard)> {
        // When creating a new RtpSender, make sure we're always using the latest session
        let (session, _r_lock) = {
            let (handle, _r_lock) = self.inner.wait_reconnection().await?;
            (handle.session.clone(), _r_lock)
        };

        session.create_sender(track, options, encodings).await
    }

    pub fn publisher_negotiation_needed(&self) {
        let inner = self.inner.clone();
        livekit_runtime::spawn(async move {
            if let Ok((handle, _)) = inner.wait_reconnection().await {
                handle.session.publisher_negotiation_needed()
            }
        });
    }

    pub async fn send_request(&self, msg: proto::signal_request::Message) {
        // Getting the current session is OK to do without waiting for reconnection
        // SignalClient will attempt to queue the message if the session is not connected
        // Also on full_reconnect, every message is OK to ignore (Since this is another RtcSession)
        let session = self.inner.running_handle.read().session.clone();
        session.signal_client().send(msg).await // Returns () and automatically queues the message
                                                // on fail
    }

    pub async fn get_response(&self, request_id: u32) -> proto::RequestResponse {
        let session = self.inner.running_handle.read().session.clone();
        session.get_response(request_id).await
    }

    pub async fn get_stats(&self) -> EngineResult<SessionStats> {
        let session = self.inner.running_handle.read().session.clone();
        session.get_stats().await
    }

    pub fn session(&self) -> Arc<RtcSession> {
        self.inner.running_handle.read().session.clone()
    }
}

impl EngineInner {
    async fn connect(
        url: &str,
        token: &str,
        options: EngineOptions,
        e2ee_manager: Option<E2eeManager>,
    ) -> EngineResult<(Arc<Self>, proto::JoinResponse, EngineEvents)> {
        let lk_runtime = LkRuntime::instance();
        let max_retries = options.join_retries;

        let try_connect = {
            move || {
                let options = options.clone();
                let lk_runtime = lk_runtime.clone();
                let e2ee_manager = e2ee_manager.clone();
                async move {
                    let (session, join_response, session_events) =
                        RtcSession::connect(url, token, options.clone(), e2ee_manager).await?;
                    session.wait_pc_connection().await?;

                    let (engine_tx, engine_rx) = mpsc::unbounded_channel();
                    let mut interval = interval(RECONNECT_INTERVAL);
                    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
                    let inner = Arc::new(Self {
                        lk_runtime,
                        engine_tx,
                        close_notifier: Arc::new(Notify::new()),
                        running_handle: RwLock::new(EngineHandle {
                            session: Arc::new(session),
                            closed: false,
                            reconnecting: false,
                            can_reconnect: true,
                            full_reconnect: false,
                            engine_task: None,
                        }),
                        options,
                        reconnecting_lock: AsyncRwLock::default(),
                        reconnecting_interval: AsyncMutex::new(interval),
                    });

                    // Start initial tasks
                    let (close_tx, close_rx) = oneshot::channel();
                    let session_task = livekit_runtime::spawn(Self::engine_task(
                        inner.clone(),
                        session_events,
                        close_rx,
                    ));
                    inner.running_handle.write().engine_task = Some((session_task, close_tx));

                    Ok((inner, join_response, engine_rx))
                }
            }
        };

        let mut last_error = None;
        for i in 0..(max_retries + 1) {
            match try_connect().await {
                Ok(res) => return Ok(res),
                Err(e) => {
                    let attempt_i = i + 1;
                    if i < max_retries {
                        log::warn!(
                            "failed to connect: {:?}, retrying... ({}/{})",
                            e,
                            attempt_i,
                            max_retries
                        );
                    }
                    last_error = Some(e)
                }
            }
        }

        Err(last_error.unwrap())
    }

    async fn engine_task(
        self: Arc<Self>,
        mut session_events: SessionEvents,
        mut close_rx: oneshot::Receiver<()>,
    ) {
        loop {
            tokio::select! {
                Some(event) = session_events.recv() => {
                    let debug = format!("{:?}", event);
                    let inner = self.clone();
                    let (tx, rx) = oneshot::channel();
                    let task = livekit_runtime::spawn(async move {
                        if let Err(err) = inner.on_session_event(event).await {
                            log::error!("failed to handle session event: {:?}", err);
                        }
                        let _ = tx.send(());
                    });

                    // Monitor sync/async blockings
                    tokio::select! {
                        _ = rx => {},
                        _ = livekit_runtime::sleep(Duration::from_secs(10)) => {
                            log::error!("session_event is taking too much time: {}", debug);
                        }
                    }

                    task.await;
                },
                 _ = &mut close_rx => {
                    break;
                }
            }
        }

        log::debug!("engine task closed");
    }

    async fn on_session_event(self: &Arc<Self>, event: SessionEvent) -> EngineResult<()> {
        match event {
            SessionEvent::Close { source, reason, action, retry_now } => {
                match action {
                    proto::leave_request::Action::Resume
                    | proto::leave_request::Action::Reconnect => {
                        {
                            let running_handle = self.running_handle.read();

                            // server could have sent a leave & disconnected signal client
                            // we don't want to start another resume cycle
                            if !running_handle.can_reconnect {
                                return Ok(());
                            }
                            // ensure we release the lock from this scope, it'll be used again in reconnection_needed
                        }

                        log::warn!(
                            "received session close: {:?} {:?} {:?}",
                            source,
                            reason,
                            action
                        );
                        self.reconnection_needed(
                            retry_now,
                            action == proto::leave_request::Action::Reconnect,
                        );
                    }
                    proto::leave_request::Action::Disconnect => {
                        // Disallow reconnection to avoid races
                        let mut running_handle = self.running_handle.write();
                        running_handle.can_reconnect = false;

                        // Spawning a new task because the close function wait for the engine_task to
                        // finish. (So it doesn't make sense to await it here)
                        livekit_runtime::spawn({
                            let inner = self.clone();
                            async move {
                                inner.close(reason).await;
                            }
                        });
                    }
                }
            }
            SessionEvent::Data {
                participant_sid,
                participant_identity,
                payload,
                topic,
                kind,
                encryption_type,
            } => {
                let _ = self.engine_tx.send(EngineEvent::Data {
                    participant_sid,
                    participant_identity,
                    payload,
                    topic,
                    kind,
                    encryption_type,
                });
            }
            SessionEvent::ChatMessage { participant_identity, message } => {
                let _ =
                    self.engine_tx.send(EngineEvent::ChatMessage { participant_identity, message });
            }
            SessionEvent::SipDTMF { participant_identity, code, digit } => {
                let _ =
                    self.engine_tx.send(EngineEvent::SipDTMF { participant_identity, code, digit });
            }
            SessionEvent::Transcription { participant_identity, track_sid, segments } => {
                let _ = self.engine_tx.send(EngineEvent::Transcription {
                    participant_identity,
                    track_sid,
                    segments,
                });
            }
            SessionEvent::SipDTMF { participant_identity, code, digit } => {
                let _ =
                    self.engine_tx.send(EngineEvent::SipDTMF { participant_identity, code, digit });
            }
            SessionEvent::RpcRequest {
                caller_identity,
                request_id,
                method,
                payload,
                response_timeout,
                version,
            } => {
                let _ = self.engine_tx.send(EngineEvent::RpcRequest {
                    caller_identity,
                    request_id,
                    method,
                    payload,
                    response_timeout,
                    version,
                });
            }
            SessionEvent::RpcResponse { request_id, payload, error } => {
                let _ =
                    self.engine_tx.send(EngineEvent::RpcResponse { request_id, payload, error });
            }
            SessionEvent::RpcAck { request_id } => {
                let _ = self.engine_tx.send(EngineEvent::RpcAck { request_id });
            }
            SessionEvent::MediaTrack { track, stream, transceiver } => {
                let _ = self.engine_tx.send(EngineEvent::MediaTrack { track, stream, transceiver });
            }
            SessionEvent::ParticipantUpdate { updates } => {
                let _ = self.engine_tx.send(EngineEvent::ParticipantUpdate { updates });
            }
            SessionEvent::SpeakersChanged { speakers } => {
                let _ = self.engine_tx.send(EngineEvent::SpeakersChanged { speakers });
            }
            SessionEvent::ConnectionQuality { updates } => {
                let _ = self.engine_tx.send(EngineEvent::ConnectionQuality { updates });
            }
            SessionEvent::RoomUpdate { room } => {
                let _ = self.engine_tx.send(EngineEvent::RoomUpdate { room });
            }
            SessionEvent::RoomMoved { moved } => {
                let _ = self.engine_tx.send(EngineEvent::RoomMoved { moved });
            }
            SessionEvent::LocalTrackSubscribed { track_sid } => {
                let _ = self.engine_tx.send(EngineEvent::LocalTrackSubscribed { track_sid });
            }
            SessionEvent::DataStreamHeader { header, participant_identity, encryption_type } => {
                let _ = self.engine_tx.send(EngineEvent::DataStreamHeader {
                    header,
                    participant_identity,
                    encryption_type,
                });
            }
            SessionEvent::DataStreamChunk { chunk, participant_identity, encryption_type } => {
                let _ = self.engine_tx.send(EngineEvent::DataStreamChunk {
                    chunk,
                    participant_identity,
                    encryption_type,
                });
            }
            SessionEvent::DataStreamTrailer { trailer, participant_identity } => {
                let _ = self
                    .engine_tx
                    .send(EngineEvent::DataStreamTrailer { trailer, participant_identity });
            }
            SessionEvent::DataChannelBufferedAmountLowThresholdChanged { kind, threshold } => {
                let _ = self.engine_tx.send(
                    EngineEvent::DataChannelBufferedAmountLowThresholdChanged { kind, threshold },
                );
            }
            SessionEvent::RefreshToken { url, token } => {
                let _ = self.engine_tx.send(EngineEvent::RefreshToken { url, token });
            }
            SessionEvent::TrackMuted { sid, muted } => {
                let _ = self.engine_tx.send(EngineEvent::TrackMuted { sid, muted });
            }
            SessionEvent::SubscribedQualityUpdate { update } => {
                let _ = self.engine_tx.send(EngineEvent::SubscribedQualityUpdate { update });
            }
            SessionEvent::LocalDataTrackInput(event) => {
                let _ = self.engine_tx.send(EngineEvent::LocalDataTrackInput(event));
            }
            SessionEvent::RemoteDataTrackInput(event) => {
                let _ = self.engine_tx.send(EngineEvent::RemoteDataTrackInput(event));
            }
        }
        Ok(())
    }

    /// Close the engine
    /// the RtcSession is not removed so we can still access stats for e.g
    async fn close(&self, reason: DisconnectReason) {
        let (session, engine_task) = {
            let mut running_handle = self.running_handle.write();
            running_handle.closed = true;

            let session = running_handle.session.clone();
            let engine_task = running_handle.engine_task.take();
            (session, engine_task)
        };
        // TINHAT PATCH (reconnect-wedge): upstream never fired close_notifier, so closing the room left a
        // running reconnect alive: a wedged resume could come back later with the same identity and the SFU
        // closed the NEWER session (the rejoined call) with DUPLICATE_IDENTITY. Cancel it now. close() is
        // never called from inside reconnect_task itself (its give-up reasons go back to the caller).
        self.close_notifier.notify_waiters();

        if let Some((engine_task, close_tx)) = engine_task {
            bounded(SESSION_CLOSE_TIMEOUT, "closing the session", session.close(reason)).await;
            let _ = close_tx.send(());
            bounded(SESSION_CLOSE_TIMEOUT, "stopping the engine task", engine_task).await;
        } else {
            // TINHAT PATCH (Astra r2): a restart took the engine task and was closing this session when we cancelled it:
            // close it here (a second close of a closed session only closes what is still open)
            bounded(SESSION_CLOSE_TIMEOUT, "closing the session", session.close(reason)).await;
        }

        // Always emit Disconnected, even when the engine_task was already taken by a
        // prior failed `try_restart_connection`. Without this, a reconnect cycle that
        // exhausts all attempts leaves the room stuck in Reconnecting forever because
        // the room's task never sees the event that drives `handle_disconnected`.
        let _ = self.engine_tx.send(EngineEvent::Disconnected { reason });
    }

    /// When waiting for reconnection, it ensures we're always using the latest session.
    async fn wait_reconnection(
        &self,
    ) -> EngineResult<(RwLockReadGuard<EngineHandle>, AsyncRwLockReadGuard<()>)> {
        let r_lock = self.reconnecting_lock.read().await;
        let running_handle = self.running_handle.read();

        if running_handle.closed {
            // Reconnection may have failed
            // TODO(theomonnom): More precise error?
            return Err(EngineError::Connection("engine is closed".into()));
        }

        Ok((running_handle, r_lock))
    }

    /// Start the reconnect task if not already started
    /// Ask to retry directly if `retry_now` is true
    /// Ask for a full reconnect if `full_reconnect` is true
    fn reconnection_needed(self: &Arc<Self>, retry_now: bool, full_reconnect: bool) {
        let mut running_handle = self.running_handle.write();

        if !running_handle.can_reconnect {
            return;
        }

        if running_handle.reconnecting {
            // If we're already reconnecting just update the interval to restart a new attempt
            // ASAP

            // Only escalate to full reconnect, never downgrade. Stale signal-close
            // events (which request resume) must not override a full reconnect decision
            // made by the reconnect loop after a failed resume attempt.
            if full_reconnect {
                running_handle.full_reconnect = true;
            }

            if retry_now {
                let inner = self.clone();
                livekit_runtime::spawn(async move {
                    inner.reconnecting_interval.lock().await.reset();
                });
            }

            return;
        }

        running_handle.reconnecting = true;
        running_handle.full_reconnect = full_reconnect;

        livekit_runtime::spawn({
            let inner = self.clone();
            async move {
                // The close function can send a signal to cancel the reconnection.
                // TINHAT PATCH (reconnect-wedge): listen BEFORE waiting for the lock (Notified receives
                // notify_waiters from its creation on), so a close() while we wait for it cancels us too.
                let close_notifier = inner.close_notifier.clone();
                let close_receiver = close_notifier.notified();
                tokio::pin!(close_receiver);

                // Hold the reconnection lock for the whole reconnection time
                let _r_lock = tokio::select! {
                    _ = &mut close_receiver => {
                        log::debug!("reconnection cancelled before it started");
                        return;
                    }
                    l = inner.reconnecting_lock.write() => l,
                };

                tokio::select! {
                    _ = &mut close_receiver => {
                        log::debug!("reconnection cancelled");
                        return;
                    }
                    res = inner.reconnect_task() => {
                        if let Err(reason) = res {
                            log::error!("failed to reconnect to the livekit room");
                            inner.close(reason).await;
                        } else {
                            log::info!("RtcEngine successfully recovered")
                        }
                    }
                }

                let mut running_handle = inner.running_handle.write();
                running_handle.reconnecting = false;

                // r_lock is now dropped
            }
        });
    }

    /// Runned every time the PeerConnection or the SignalClient is closed
    /// We first try to resume the connection, if it fails, we start a full reconnect.
    /// NOTE: The reconnect_task must be canncellation safe
    /// TINHAT PATCH (reconnect-wedge): Err carries the reason the caller closes the engine with (the
    /// server's for a Leave, UnknownReason otherwise); reconnect_task never calls close() itself, so the
    /// close notification can't cancel it halfway through its own close.
    async fn reconnect_task(self: &Arc<Self>) -> Result<(), DisconnectReason> {
        // Get the latest connection info from the signal_client (including the refreshed token
        // because the initial join token may have expired)
        let (url, token, e2ee_manager) = {
            let running_handle = self.running_handle.read();
            let signal_client = running_handle.session.signal_client();
            let e2ee_manager = running_handle.session.e2ee_manager();
            (
                signal_client.url(),
                signal_client.token(), // Refreshed token
                e2ee_manager.clone(),
            )
        };

        for i in 1..=RECONNECT_ATTEMPTS {
            let (is_closed, full_reconnect) = {
                let running_handle = self.running_handle.read();
                (running_handle.closed, running_handle.full_reconnect)
            };

            if is_closed {
                log::warn!("reconnect attempt cancelled, engine is closed");
                return Err(DisconnectReason::UnknownReason);
            }

            if full_reconnect {
                if i == 1 {
                    let (tx, rx) = oneshot::channel();
                    let _ = self.engine_tx.send(EngineEvent::Restarting(tx));
                    bounded(ROOM_HANDSHAKE_TIMEOUT, "the room's Restarting", rx).await;
                }

                log::error!("restarting connection... attempt: {}", i);
                let attempt = bounded(
                    RESTART_ATTEMPT_TIMEOUT,
                    "a restart attempt",
                    self.try_restart_connection(&url, &token, self.options.clone(), e2ee_manager.clone()),
                )
                .await
                .unwrap_or_else(|| Err(EngineError::Connection("restart attempt timed out".into())));
                match attempt {
                    Ok(()) => {
                        let (tx, rx) = oneshot::channel();
                        let _ = self.engine_tx.send(EngineEvent::Restarted(tx));
                        bounded(ROOM_HANDSHAKE_TIMEOUT, "the room's Restarted", rx).await;
                        return Ok(());
                    }
                    Err(err) => {
                        if let Some(reason) = leave_disconnect_reason(&err) {
                            log::warn!("server requested disconnect during restart: {:?}", reason);
                            self.running_handle.write().can_reconnect = false;
                            return Err(reason);
                        }
                        log::error!("restarting connection failed: {}", err);
                    }
                }
            } else {
                if i == 1 {
                    let (tx, rx) = oneshot::channel();
                    let _ = self.engine_tx.send(EngineEvent::Resuming(tx));
                    bounded(ROOM_HANDSHAKE_TIMEOUT, "the room's Resuming", rx).await;
                }

                log::error!("resuming connection... attempt: {}", i);
                let attempt = bounded(RESUME_ATTEMPT_TIMEOUT, "a resume attempt", self.try_resume_connection())
                    .await
                    .unwrap_or_else(|| Err(EngineError::Connection("resume attempt timed out".into())));
                match attempt {
                    Ok(()) => {
                        let (tx, rx) = oneshot::channel();
                        let _ = self.engine_tx.send(EngineEvent::Resumed(tx));
                        bounded(ROOM_HANDSHAKE_TIMEOUT, "the room's Resumed", rx).await;
                        return Ok(());
                    }
                    Err(err) => {
                        if let Some(reason) = leave_disconnect_reason(&err) {
                            log::warn!("server requested disconnect during resume: {:?}", reason);
                            self.running_handle.write().can_reconnect = false;
                            return Err(reason);
                        }
                        log::error!("resuming connection failed: {}", err);
                        let mut running_handle = self.running_handle.write();
                        running_handle.full_reconnect = true;
                    }
                }
            }

            self.reconnecting_interval.lock().await.tick().await;
        }

        log::error!("failed to reconnect after {} attempts", RECONNECT_ATTEMPTS);
        Err(DisconnectReason::UnknownReason)
    }

    /// Try to recover the connection by doing a full reconnect.
    /// It recreates a new RtcSession (new peer connection, new signal client, new data channels,
    /// etc...)
    async fn try_restart_connection(
        self: &Arc<Self>,
        url: &str,
        token: &str,
        options: EngineOptions,
        e2ee_manager: Option<E2eeManager>,
    ) -> EngineResult<()> {
        // Close the current RtcSession and the current tasks
        let (session, engine_task) = {
            let mut running_handle = self.running_handle.write();
            let session = running_handle.session.clone();
            let engine_task = running_handle.engine_task.take();
            (session, engine_task)
        };

        if let Some((engine_task, close_tx)) = engine_task {
            // TINHAT PATCH (reconnect-wedge): the old session's link may be dead; never wait on it forever
            bounded(SESSION_CLOSE_TIMEOUT, "closing the old session", session.close(DisconnectReason::ClientInitiated)).await;
            let _ = close_tx.send(());
            bounded(SESSION_CLOSE_TIMEOUT, "stopping the old engine task", engine_task).await;
        }

        let (new_session, join_response, session_events) =
            RtcSession::connect(url, token, options, e2ee_manager).await?;
        let new_session = ProvisionalSession(Some(new_session));

        // On SignalRestarted, the room will try to unpublish the local tracks
        // NOTE: Doing operations that use rtc_session will not use the new one
        let (tx, rx) = oneshot::channel();
        let _ = self.engine_tx.send(EngineEvent::SignalRestarted { join_response, tx });
        // TINHAT PATCH (Astra r1 P2): no answer = a failed attempt, never "the room took it"
        if !matches!(bounded(ROOM_HANDSHAKE_TIMEOUT, "the room's SignalRestarted", rx).await, Some(Ok(()))) {
            return Err(EngineError::Connection("the room did not take the restarted signal".into()));
        }

        new_session.get().wait_pc_connection().await?;

        // Only replace the current session if the new one succeed
        // This is important so we can still use the old session if the new one failed
        // (for example, this is important if we still want to get the stats of the old session)
        // This has the drawback to not being able to use the new session on the SignalRestarted
        // event.
        let mut handle = self.running_handle.write();
        handle.session = Arc::new(new_session.commit());

        let (close_tx, close_rx) = oneshot::channel();
        let task = livekit_runtime::spawn(self.clone().engine_task(session_events, close_rx));
        handle.engine_task = Some((task, close_tx));

        Ok(())
    }

    /// Try to restart the current session
    async fn try_resume_connection(&self) -> EngineResult<()> {
        let session = self.running_handle.read().session.clone();
        let reconnect_response = session.restart().await?;

        let (tx, rx) = oneshot::channel();
        let _ = self.engine_tx.send(EngineEvent::SignalResumed { reconnect_response, tx });

        // With SignalResumed, the room will send a SyncState message to the server.
        // SyncState is a pass-through signal so it goes out immediately even though
        // the SignalClient is still in `reconnecting=true` state.
        // TINHAT PATCH (Astra r1 P2): no answer = no SyncState sent; the publisher offer must not go before it
        if !matches!(bounded(ROOM_HANDSHAKE_TIMEOUT, "the room's SignalResumed", rx).await, Some(Ok(()))) {
            return Err(EngineError::Connection("the room did not send its sync state".into()));
        }

        // The publisher offer must be sent AFTER the SyncState message
        session.restart_publisher().await?;
        session.wait_pc_reconnected(PC_RECONNECT_SETTLE_DELAY).await?;

        // Re-check the signal connection BEFORE flushing the queue. If the WS died
        // while we were waiting for PCs to come back, draining queued mutations
        // would just push them into the void; better to bail and let the engine
        // try a fresh resume (or escalate).
        if !session.signal_client().is_connected().await {
            return Err(EngineError::Connection("signal connection severed during resume".into()));
        }

        // Flush queued mutations and clear the `reconnecting` flag — at this point
        // the resume has fully recovered, so deferred subscription updates / mutes
        // / etc. should now reach the server. Mirrors `client.setReconnected()`.
        session.signal_client().set_reconnected().await;

        Ok(())
    }
}

impl From<livekit_datatrack::api::InternalError> for EngineError {
    fn from(err: livekit_datatrack::api::InternalError) -> Self {
        Self::Internal(err.to_string().into())
    }
}

/// Inspect a reconnect-attempt error and return the server-supplied disconnect
/// reason iff the server sent `LeaveRequest{action: Disconnect}` while we were
/// trying to (re)connect. In that case the reconnect loop should bail out
/// rather than escalate to a full reconnect — the server is explicitly telling
/// us to stop trying. `Reconnect`/`Resume` actions still fall through to the
/// normal escalation path.
fn leave_disconnect_reason(err: &EngineError) -> Option<DisconnectReason> {
    if let EngineError::Signal(SignalError::LeaveRequest { reason, action }) = err {
        if *action == proto::leave_request::Action::Disconnect {
            return Some(*reason);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leave_disconnect_reason_returns_some_only_for_disconnect_action() {
        let disconnect_err = EngineError::Signal(SignalError::LeaveRequest {
            reason: DisconnectReason::ServerShutdown,
            action: proto::leave_request::Action::Disconnect,
        });
        assert_eq!(
            leave_disconnect_reason(&disconnect_err),
            Some(DisconnectReason::ServerShutdown),
            "Disconnect action should propagate the server reason"
        );

        for action in
            [proto::leave_request::Action::Reconnect, proto::leave_request::Action::Resume]
        {
            let err = EngineError::Signal(SignalError::LeaveRequest {
                reason: DisconnectReason::ServerShutdown,
                action,
            });
            assert!(
                leave_disconnect_reason(&err).is_none(),
                "{:?} action must NOT short-circuit the reconnect loop",
                action
            );
        }
    }

    #[test]
    fn leave_disconnect_reason_ignores_non_leave_errors() {
        let other_errors = [
            EngineError::Connection("network".into()),
            EngineError::Internal("bug".into()),
            EngineError::Signal(SignalError::SendError),
            EngineError::Signal(SignalError::Timeout("waiting".into())),
        ];
        for err in &other_errors {
            assert!(
                leave_disconnect_reason(err).is_none(),
                "{:?} must not be treated as a disconnect Leave",
                err
            );
        }
    }
}
