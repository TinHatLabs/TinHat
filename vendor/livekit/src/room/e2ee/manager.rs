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

use std::{collections::HashMap, sync::Arc};

use libwebrtc::{
    native::{
        frame_cryptor::{
            DataPacketCryptor, EncryptedPacket, EncryptionAlgorithm, EncryptionState, FrameCryptor,
        },
        packet_trailer,
    },
    rtp_receiver::RtpReceiver,
    rtp_sender::RtpSender,
};
use livekit_protocol::PacketTrailerFeature;
use parking_lot::{Condvar, Mutex};

use super::{key_provider::KeyProvider, EncryptionType};
use crate::{
    e2ee::E2eeOptions,
    id::{ParticipantIdentity, TrackSid},
    participant::{LocalParticipant, RemoteParticipant},
    prelude::{LocalTrack, LocalTrackPublication, RemoteTrack, RemoteTrackPublication},
    rtc_engine::lk_runtime::LkRuntime,
};
use std::fmt::Debug;

type StateChangedHandler = Box<dyn Fn(ParticipantIdentity, EncryptionState) + Send>;

fn has_packet_trailer_features(features: &[i32]) -> bool {
    features.iter().any(|f| {
        *f == PacketTrailerFeature::PtfUserTimestamp as i32
            || *f == PacketTrailerFeature::PtfFrameId as i32
    })
}

fn needs_video_receiver_packet_trailer_handler(features: &[i32]) -> bool {
    has_packet_trailer_features(features)
}

struct ManagerInner {
    options: Option<E2eeOptions>, // If Some, it means the e2ee was initialized
    enabled: bool,                // Used to enable/disable e2ee
    dc_encryption_enabled: bool,
    frame_cryptors: HashMap<(ParticipantIdentity, TrackSid), FrameCryptor>, // RECEIVERS (remote tracks)
    // TINHAT PATCH (12.1.3, C2b): our SENDER cryptors live apart from the receivers (a forged remote
    // participant with our identity + sid can't displace or remove them), each bound to its
    // track's client id (removed only by the unpublish of THAT track). A sender cryptor is never
    // dropped while its sender may still send: one that would be replaced is RETIRED (kept alive
    // until cleanup) — dropping it detaches encryption (video then goes out unencrypted).
    sender_cryptors: HashMap<(ParticipantIdentity, TrackSid), (String, FrameCryptor)>,
    retired_senders: Vec<FrameCryptor>,
    data_packet_cryptor: Option<DataPacketCryptor>,
    // TINHAT PATCH: the key index OUR sender cryptors encrypt with. FrameCryptors are
    // created with index 0 and nothing upstream ever set it, so a local key installed
    // at any other index silently diverged from what we announce to peers. Applied in
    // setup_rtp_sender and retroactively by set_sender_key_index.
    sender_key_index: i32,
    // TINHAT PATCH (early receiver attach): participant sid -> identity, fed by the session from
    // the join response and every participant update (both arrive before the subscriber offer
    // that announces their tracks).
    participant_sids: HashMap<String, ParticipantIdentity>,
    // Every receiver cryptor, keyed by its NATIVE receiver object (RtpReceiver::native_key; not
    // the receiver's track id, which the SFU's SDP chooses and may repeat). Attached in the
    // synchronous on_track (publication None = pending its TrackSubscribed) or, as a fallback, at
    // TrackSubscribed; kept through promotion, so one receiver + identity never gets a second
    // transformer. The entry's FrameCryptor holds a reference to its receiver, so a key can't be
    // reused by another receiver while the entry exists. RECEIVERS only: our sender cryptors
    // stay in sender_cryptors (C2b).
    receivers: HashMap<usize, RxEntry>,
    // (identity, publication sid) -> receiver key, for the unsubscribe
    receiver_of: HashMap<(ParticipantIdentity, TrackSid), usize>,
    // receivers whose cryptor is being built right now (outside the lock): one builder per receiver.
    // The signaling thread (on_track) never waits for a claim — it leaves that receiver to the
    // claimant; on_track_subscribed waits (rx_built) until the claim is released — never bypasses
    // it. Each claim has its own token: only its owner (or the owner's unwind guard) releases it.
    building: HashMap<usize, (Builder, u64)>,
    next_claim: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Builder {
    OnTrack,
    Subscribe,
}

/// After this long waiting for another thread's receiver build, on_track_subscribed logs (and
/// keeps waiting: proceeding would put a second transformer on the receiver).
const RX_BUILD_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// TINHAT PATCH (early receiver attach): releases a receiver build claim if its owner unwinds
/// before registering (a released claim is a no-op here).
struct ClaimGuard<'a> {
    manager: &'a E2eeManager,
    rx_key: usize,
    token: u64,
    done: bool,
}

impl ClaimGuard<'_> {
    /// release under the caller's lock section (normal completion)
    fn release(mut self, inner: &mut ManagerInner) {
        self.done = true;
        E2eeManager::release_claim(inner, self.rx_key, self.token);
        self.manager.rx_built.notify_all();
    }
}

impl Drop for ClaimGuard<'_> {
    fn drop(&mut self) {
        if !self.done {
            E2eeManager::release_claim(&mut self.manager.inner.lock(), self.rx_key, self.token);
            self.manager.rx_built.notify_all();
        }
    }
}

struct RxEntry {
    identity: ParticipantIdentity,
    cryptor: FrameCryptor,
    publication: Option<(ParticipantIdentity, TrackSid)>, // None = early, not subscribed yet
}

/// Upper bound for receiver cryptors attached at on_track that never got their TrackSubscribed
/// (each holds a libwebrtc transformer); past it, new receivers get their cryptor at
/// TrackSubscribed (nothing pending is ever evicted).
const MAX_EARLY_RECEIVERS: usize = 256;

#[derive(Clone)]
pub struct E2eeManager {
    inner: Arc<Mutex<ManagerInner>>,
    state_changed: Arc<Mutex<Option<StateChangedHandler>>>,
    rx_built: Arc<Condvar>, // TINHAT PATCH: a receiver build claim was released
}

impl E2eeManager {
    /// E2eeOptions is an optional parameter. We may support to reconfigure e2ee after connect in
    /// the future.
    pub(crate) fn new(options: Option<E2eeOptions>, with_dc_encryption: bool) -> Self {
        // Create DataPacketCryptor whenever E2EE options are available
        // This allows for decryption even if we're not encrypting our own data
        let data_packet_cryptor = options.as_ref().map(|opts| {
            DataPacketCryptor::new(EncryptionAlgorithm::AesGcm, opts.key_provider.handle.clone())
        });

        Self {
            inner: Arc::new(Mutex::new(ManagerInner {
                enabled: options.is_some(), // Enabled by default if options is provided
                dc_encryption_enabled: options.is_some() && with_dc_encryption,
                options,
                frame_cryptors: HashMap::new(),
                sender_cryptors: HashMap::new(),
                retired_senders: Vec::new(),
                data_packet_cryptor,
                sender_key_index: 0,
                participant_sids: HashMap::new(),
                receivers: HashMap::new(),
                receiver_of: HashMap::new(),
                building: HashMap::new(),
                next_claim: 0,
            })),
            state_changed: Default::default(),
            rx_built: Default::default(),
        }
    }

    /// TINHAT PATCH: set the key index every LOCAL sender cryptor encrypts with — both the
    /// ones that already exist and any created by future publishes/reconnects. Must match
    /// the index the local key was installed under in the KeyProvider (and the index
    /// announced to peers), or peers install a key the frames never reference.
    pub fn set_sender_key_index(&self, _local_identity: &ParticipantIdentity, index: i32) {
        let mut inner = self.inner.lock();
        inner.sender_key_index = index;
        // C2b: every sender cryptor is ours (separate map), retired ones included
        for (_, cryptor) in inner.sender_cryptors.values() {
            cryptor.set_key_index(index);
        }
        for cryptor in inner.retired_senders.iter() {
            cryptor.set_key_index(index);
        }
    }

    pub(crate) fn cleanup(&self) {
        // the room closed its PeerConnections first (no sender sends any more); disable + drop
        // outside the lock (a sender's destructor detaches via libwebrtc's signaling thread, where
        // early_attach_receiver may be waiting for this lock)
        let (receivers, senders, retired, rx) = {
            let mut inner = self.inner.lock();
            inner.participant_sids.clear();
            inner.receiver_of.clear();
            (
                std::mem::take(&mut inner.frame_cryptors),
                std::mem::take(&mut inner.sender_cryptors),
                std::mem::take(&mut inner.retired_senders),
                std::mem::take(&mut inner.receivers),
            )
        };
        for cryptor in receivers
            .values()
            .chain(senders.values().map(|(_, c)| c))
            .chain(retired.iter())
            .chain(rx.values().map(|e| &e.cryptor))
        {
            cryptor.set_enabled(false);
        }
        drop((receivers, senders, retired, rx));
    }

    /// TINHAT PATCH (early receiver attach): a new signal session (join / full restart) starts
    /// from its own join response: forget the previous session's sids and its never-subscribed
    /// early receiver cryptors (their PeerConnection is closed before a new session connects).
    /// Subscribed receivers and every sender cryptor are left to the room's own unsubscribe /
    /// unpublish / cleanup (their keys can't collide with the new session's receivers: each entry
    /// holds its own receiver alive).
    pub(crate) fn reset_session(&self) {
        let early: Vec<RxEntry> = {
            let mut inner = self.inner.lock();
            inner.participant_sids.clear();
            let keys: Vec<usize> =
                inner.receivers.iter().filter(|(_, e)| e.publication.is_none()).map(|(k, _)| *k).collect();
            keys.into_iter().filter_map(|k| inner.receivers.remove(&k)).collect()
        };
        drop(early); // outside the lock (see cleanup); receivers are never detached natively
    }

    /// Register to e2ee state changes
    /// Used by the room to dispatch the event to the room dispatcher
    pub(crate) fn on_state_changed(
        &self,
        handler: impl Fn(ParticipantIdentity, EncryptionState) + Send + 'static,
    ) {
        *self.state_changed.lock() = Some(Box::new(handler));
    }

    pub(crate) fn initialized(&self) -> bool {
        self.inner.lock().options.is_some()
    }

    pub(crate) fn on_track_subscribed(
        &self,
        track: RemoteTrack,
        publication: RemoteTrackPublication,
        participant: RemoteParticipant,
    ) {
        let identity = participant.identity();
        let receiver = track.transceiver().unwrap().receiver();

        let needs_packet_trailer_handler = needs_video_receiver_packet_trailer_handler(
            &publication.proto_info().packet_trailer_features,
        );
        let e2ee = self.initialized();

        if let RemoteTrack::Video(video_track) = &track {
            if needs_packet_trailer_handler && e2ee {
                // TINHAT PATCH (early receiver attach): the trailer features are the SFU's claim
                // (TrackInfo). Creating the receiver handler REPLACES the receiver's transformer
                // (our cryptor, attached at on_track) with a trailer-only one until
                // set_packet_trailer_handler re-chains the two — an SFU advertising them would
                // reopen the window in which frames reach the decoder without decryption, and
                // replace a video transformer. No Tin Hat client publishes trailers, so in an
                // E2EE room the receiver handler is never created.
                log::warn!(
                    "remote track {} from {} advertises packet trailer features: ignored in an E2EE room",
                    publication.sid(),
                    identity,
                );
            } else if needs_packet_trailer_handler {
                let handler = packet_trailer::create_receiver_handler(
                    LkRuntime::instance().pc_factory(),
                    &receiver,
                );
                // (non-E2EE room: no cryptor to chain it with; on_track_subscribed returns below)
                video_track.set_packet_trailer_handler(handler);
                log::info!(
                    "attached packet_trailer handler for subscribed track {} from {}",
                    publication.sid(),
                    identity,
                );
            } else {
                log::debug!(
                    "skipping packet_trailer handler for subscribed track {} from {} without advertised packet trailer support",
                    publication.sid(),
                    identity,
                );
            }
        }

        if !e2ee {
            return;
        }
        // TINHAT PATCH (never-trust-NONE): the advertised encryption type comes from the SFU
        // (TrackInfo), so skipping the cryptor for a publication that claims "none" would let a
        // hostile call host play its own media as this participant. In an E2EE room every remote
        // publication gets a cryptor; frames that don't decrypt are dropped (the key provider's
        // discard_frame_when_cryptor_not_ready + missing-key/IV checks), never played.
        if publication.encryption_type() == EncryptionType::None {
            log::warn!(
                "remote track {} from {} is advertised as UNENCRYPTED: attaching a cryptor anyway (not played unless it decrypts)",
                publication.sid(),
                identity,
            );
            eprintln!(
                "[e2ee] {} track {} is advertised as UNENCRYPTED: not played (Tin Hat plays only end-to-end encrypted media)",
                identity,
                publication.sid()
            );
        }

        // TINHAT PATCH (early receiver attach): reuse the cryptor attached to this receiver at
        // on_track (it has been dropping/decrypting since its early attachment). Lookup and promotion
        // happen in ONE lock section (nothing can change the entry in between); a receiver
        // without a cryptor for this identity gets one built here under a per-receiver build
        // claim, so on_track can't attach a second transformer meanwhile.
        let rx_key = receiver.native_key();
        let key = (identity.clone(), publication.sid());
        let mut inner = self.inner.lock();
        let mut warned = false;
        while inner.building.contains_key(&rx_key) {
            // the mutex is released while waiting; the claimant never needs this thread
            if self.rx_built.wait_for(&mut inner, RX_BUILD_WAIT).timed_out() && !warned {
                warned = true;
                log::warn!("receiver of track {}: still waiting for another cryptor build", key.1);
            }
        }
        let reusable = match inner.receivers.get(&rx_key) {
            Some(e) if e.identity == identity => Some(e.cryptor.clone()),
            Some(e) => {
                log::warn!(
                    "receiver of track {} has a cryptor for {} but belongs to {}: new cryptor",
                    key.1,
                    e.identity,
                    identity,
                );
                None
            }
            None => None,
        };
        let frame_cryptor = match reusable {
            Some(fc) => fc,
            None => {
                let claim = self.claim(&mut inner, rx_key, Builder::Subscribe);
                drop(inner);
                let fc = self.setup_rtp_receiver(&identity, receiver); // lock not held
                self.setup_cryptor(&fc);
                inner = self.inner.lock();
                claim.release(&mut inner);
                fc
            }
        };
        frame_cryptor.set_enabled(inner.enabled); // the CURRENT state, not a stale snapshot
        let replaced = Self::promote(&mut inner, rx_key, identity, key, frame_cryptor);
        drop(inner);
        drop(replaced); // outside the lock
    }

    /// Claim receiver `rx_key` for building its cryptor (caller holds the lock and checked that
    /// nobody else has).
    fn claim(&self, inner: &mut ManagerInner, rx_key: usize, by: Builder) -> ClaimGuard<'_> {
        inner.next_claim += 1;
        let token = inner.next_claim;
        inner.building.insert(rx_key, (by, token));
        ClaimGuard { manager: self, rx_key, token, done: false }
    }

    fn release_claim(inner: &mut ManagerInner, rx_key: usize, token: u64) {
        if inner.building.get(&rx_key).is_some_and(|(_, t)| *t == token) {
            inner.building.remove(&rx_key);
        }
    }

    /// Register `cryptor` as the one of receiver `rx_key`, subscribed as publication `key`
    /// (caller holds the lock). The publication's previous receiver entry (a resubscription on
    /// another receiver without an unsubscribe in between) is let go, so no entry outlives its
    /// mapping; returns what was replaced, to be dropped after the lock is released.
    fn promote(
        inner: &mut ManagerInner,
        rx_key: usize,
        identity: ParticipantIdentity,
        key: (ParticipantIdentity, TrackSid),
        cryptor: FrameCryptor,
    ) -> Vec<FrameCryptor> {
        let mut replaced = Vec::new();
        if let Some(prev_rx) = inner.receiver_of.insert(key.clone(), rx_key) {
            if prev_rx != rx_key
                && inner.receivers.get(&prev_rx).is_some_and(|e| e.publication.as_ref() == Some(&key))
            {
                replaced.extend(inner.receivers.remove(&prev_rx).map(|e| e.cryptor));
            }
        }
        let old = inner.receivers.insert(
            rx_key,
            RxEntry { identity, cryptor: cryptor.clone(), publication: Some(key.clone()) },
        );
        if let Some(old) = old {
            // this receiver was subscribed as another publication: that one no longer maps here,
            // and its cryptor goes with it (nothing keeps the receiver alive outside an entry)
            if let Some(p) = old.publication.as_ref().filter(|p| **p != key) {
                if inner.receiver_of.get(p) == Some(&rx_key) {
                    inner.receiver_of.remove(p);
                    replaced.extend(inner.frame_cryptors.remove(p));
                }
            }
            replaced.push(old.cryptor);
        }
        replaced.extend(inner.frame_cryptors.insert(key, cryptor));
        replaced
    }

    /// TINHAT PATCH (early receiver attach): remember who each participant sid is. Called by the
    /// session with the join response's participants and every participant update.
    pub(crate) fn note_participants<'a>(
        &self,
        infos: impl IntoIterator<Item = (&'a str, &'a str, bool)>, // (sid, identity, disconnected)
    ) {
        let mut inner = self.inner.lock();
        if inner.options.is_none() {
            return;
        }
        for (sid, identity, gone) in infos {
            if sid.is_empty() {
                continue;
            }
            if gone {
                inner.participant_sids.remove(sid); // by sid: a disconnect needs no identity
            } else if !identity.is_empty() {
                inner.participant_sids.insert(sid.to_owned(), ParticipantIdentity(identity.to_owned()));
            }
        }
    }

    /// TINHAT PATCH (early receiver attach): called from a PeerConnection's synchronous on_track
    /// callback (libwebrtc's signaling thread, inside SetRemoteDescription). Attaches the
    /// receiver's FrameCryptor at once instead of after the room's async TrackSubscribed (before
    /// this, the first frames of a subscription reached the decoder untransformed: measured, the
    /// first ciphertext keyframe reached FFmpeg in 3 of 36 no-key runs; a hostile SFU could
    /// have sent valid plaintext there). `stream_id` = LiveKit's "PA_<participant sid>|TR_<track
    /// sid>". An unknown participant sid leaves it to on_track_subscribed (as before).
    ///
    /// Earlier protection, not a proven boundary: libwebrtc sets up the media channel before it
    /// calls OnTrack, and the stream id / sid mapping are the SFU's (see docs/CALLS-SECURITY.md).
    pub(crate) fn early_attach_receiver(&self, stream_id: &str, receiver: RtpReceiver) {
        let rx_key = receiver.native_key();
        let participant_sid = stream_id.split('|').next().unwrap_or_default();
        let (identity, claim) = {
            let mut inner = self.inner.lock();
            if inner.options.is_none() {
                return;
            }
            let Some(identity) = inner.participant_sids.get(participant_sid) else {
                log::debug!("early receiver attach: unknown participant {participant_sid}, cryptor at TrackSubscribed");
                return;
            };
            if inner.receivers.get(&rx_key).is_some_and(|e| &e.identity == identity) {
                return; // this receiver already has a cryptor for this identity (pending or subscribed)
            }
            if let Some((b, _)) = inner.building.get(&rx_key) {
                // never wait here (the claimant may need this thread); it attaches the cryptor
                log::debug!("early receiver attach: a cryptor for this receiver is being built ({b:?})");
                return;
            }
            let pending = inner.receivers.values().filter(|e| e.publication.is_none()).count();
            if pending >= MAX_EARLY_RECEIVERS {
                // never evict (dropping a wrapper doesn't free the native transformer, and its
                // subscription would then replace it): past the cap, the TrackSubscribed path
                log::warn!("early receiver attach: {MAX_EARLY_RECEIVERS} pending, cryptor at TrackSubscribed");
                return;
            }
            let identity = identity.clone();
            let claim = self.claim(&mut inner, rx_key, Builder::OnTrack); // same lock section as the checks
            (identity, claim)
        };
        let frame_cryptor = self.setup_rtp_receiver(&identity, receiver); // lock not held
        self.setup_cryptor(&frame_cryptor); // observer right after construction (native state changes are posted to this thread)
        log::debug!("early receiver attach: cryptor for {identity}");
        let old = {
            let mut inner = self.inner.lock();
            claim.release(&mut inner);
            frame_cryptor.set_enabled(inner.enabled); // the current state, not a stale snapshot
            // a subscribed entry for another identity: this receiver now decrypts as `identity`
            // (its newest transformer); that publication no longer maps to it
            let mut stale = None;
            if let Some(p) = inner.receivers.get(&rx_key).and_then(|e| e.publication.clone()) {
                if inner.receiver_of.get(&p) == Some(&rx_key) {
                    inner.receiver_of.remove(&p);
                    stale = inner.frame_cryptors.remove(&p);
                }
            }
            let entry = inner.receivers.insert(rx_key, RxEntry { identity, cryptor: frame_cryptor, publication: None });
            (entry, stale)
        };
        drop(old); // outside the lock
    }

    pub(crate) fn on_local_track_published(
        &self,
        track: LocalTrack,
        publication: LocalTrackPublication,
        participant: LocalParticipant,
    ) {
        let identity = participant.identity();
        let sender = track.transceiver().unwrap().sender();

        let packet_trailer_handler = if let LocalTrack::Video(video_track) = &track {
            let handler = video_track.packet_trailer_handler();
            if handler.is_some() {
                log::info!(
                    "packet_trailer enabled for published track {} from {}",
                    publication.sid(),
                    identity,
                );
            } else {
                log::info!(
                    "packet_trailer not enabled for published track {} from {}",
                    publication.sid(),
                    identity,
                );
            }
            handler
        } else {
            None
        };

        if !self.initialized() {
            return;
        }
        // TINHAT PATCH (12.1.3, confidentiality): the local publication's encryption type is the
        // SFU's echo (the add_track answer / later track updates). Upstream skipped the sender
        // cryptor when it said NONE, so a hostile SFU (or foreign call host) could have our media
        // sent UNENCRYPTED. Whether we encrypt is OUR room's E2EE setting (initialized), never the
        // SFU's answer.
        if publication.encryption_type() == EncryptionType::None {
            log::warn!(
                "the SFU reports our track {} as UNENCRYPTED: encrypting it anyway",
                publication.sid()
            );
            eprintln!(
                "[e2ee] the SFU reports our track {} as UNENCRYPTED: encrypting it anyway (our E2EE setting decides)",
                publication.sid()
            );
        }

        let frame_cryptor = self.setup_rtp_sender(&identity, sender);
        if let Some(handler) = packet_trailer_handler.as_ref() {
            frame_cryptor.set_packet_trailer_handler(handler);
        }
        self.setup_cryptor(&frame_cryptor);

        // C2b: into the SENDER map, bound to this track's client id; never drop a displaced one
        let cid = track.rtc_track().id();
        let mut inner = self.inner.lock();
        // the CURRENT state under the lock: a key rotation / enable change during construction
        // would otherwise be missed (this cryptor wasn't in the map yet)
        frame_cryptor.set_enabled(inner.enabled);
        frame_cryptor.set_key_index(inner.sender_key_index);
        if let Some((old_cid, old)) =
            inner.sender_cryptors.insert((identity, publication.sid()), (cid.clone(), frame_cryptor))
        {
            log::warn!("sender cryptor of track {} ({old_cid}) would be replaced by {cid}: retired, kept alive", publication.sid());
            eprintln!("[e2ee] sender cryptor of track {} ({old_cid}) would be replaced by {cid}: retired, kept alive", publication.sid());
            inner.retired_senders.push(old);
        }
    }

    fn setup_cryptor(&self, frame_cryptor: &FrameCryptor) {
        let state_changed = self.state_changed.clone();
        frame_cryptor.on_state_change(Some(Box::new(move |participant_identity, state| {
            if let Some(state_changed) = state_changed.lock().as_ref() {
                state_changed(participant_identity.into(), state);
            }
        })));
    }

    /// Called by the room
    pub(crate) fn on_local_track_unpublished(
        &self,
        publication: LocalTrackPublication,
        participant: LocalParticipant,
    ) {
        // C2b: remove the SENDER cryptor only if it belongs to THIS track (its client id): a key
        // reused meanwhile by another track's cryptor is left alone.
        let cid = publication.track().map(|t| t.rtc_track().id());
        let key = (participant.identity(), publication.sid());
        let removed = {
            let mut inner = self.inner.lock();
            match inner.sender_cryptors.get(&key) {
                Some((c, _)) if Some(c) == cid.as_ref() => inner.sender_cryptors.remove(&key),
                Some((c, _)) => {
                    log::warn!("unpublish of {} ({cid:?}): its key holds another track's cryptor ({c}), left alone", key.1);
                    None
                }
                None => None,
            }
        };
        drop(removed); // outside the lock; this track's transceiver was stopped before this callback
    }

    /// Called by the room
    pub(crate) fn on_track_unsubscribed(
        &self,
        track: RemoteTrack,
        publication: RemoteTrackPublication,
        participant: RemoteParticipant,
    ) {
        // TINHAT PATCH (early receiver attach): THIS track's receiver (a delayed unsubscribe of an
        // older receiver must not remove a newer receiver's entry for the same publication)
        let rx_key = track.transceiver().map(|t| t.receiver().native_key());
        self.remove_frame_cryptor(participant.identity(), publication.sid(), rx_key);
    }

    pub fn frame_cryptors(&self) -> HashMap<(ParticipantIdentity, TrackSid), FrameCryptor> {
        let inner = self.inner.lock();
        let mut all = inner.frame_cryptors.clone();
        for (key, (_, cryptor)) in inner.sender_cryptors.iter() {
            all.insert(key.clone(), cryptor.clone());
        }
        all
    }

    pub fn enabled(&self) -> bool {
        self.inner.lock().enabled && self.initialized()
    }

    pub fn is_dc_encryption_enabled(&self) -> bool {
        self.inner.lock().dc_encryption_enabled
    }

    pub fn set_enabled(&self, enabled: bool) {
        let mut inner = self.inner.lock();
        if inner.enabled == enabled {
            return;
        }

        inner.enabled = enabled;
        for (_, cryptor) in inner.frame_cryptors.iter() {
            cryptor.set_enabled(enabled);
        }
        for (_, cryptor) in inner.sender_cryptors.values() {
            cryptor.set_enabled(enabled);
        }
        for cryptor in inner.retired_senders.iter() {
            cryptor.set_enabled(enabled);
        }
        // TINHAT PATCH (early receiver attach): receivers attached at on_track, not subscribed yet
        for entry in inner.receivers.values().filter(|e| e.publication.is_none()) {
            entry.cryptor.set_enabled(enabled);
        }
    }

    pub fn key_provider(&self) -> Option<KeyProvider> {
        let inner = self.inner.lock();
        inner.options.as_ref().map(|opts| opts.key_provider.clone())
    }

    pub fn encryption_type(&self) -> EncryptionType {
        let inner = self.inner.lock();
        inner.options.as_ref().map(|opts| opts.encryption_type).unwrap_or(EncryptionType::None)
    }

    fn setup_rtp_sender(
        &self,
        participant_identity: &ParticipantIdentity,
        sender: RtpSender,
    ) -> FrameCryptor {
        // TINHAT PATCH (early receiver attach): no libwebrtc call under the manager lock — the
        // sender proxy runs on libwebrtc's signaling thread, where early_attach_receiver may be
        // waiting for this lock. Snapshot, build, then the insertion re-applies both under it.
        let (key_provider, index, enabled) = {
            let inner = self.inner.lock();
            (inner.options.as_ref().unwrap().key_provider.handle.clone(), inner.sender_key_index, inner.enabled)
        };

        let frame_cryptor = FrameCryptor::new_for_rtp_sender(
            LkRuntime::instance().pc_factory(),
            participant_identity.to_string(),
            EncryptionAlgorithm::AesGcm,
            key_provider,
            sender,
        );
        // TINHAT PATCH: encrypt at the app-selected index (default 0 = upstream behavior) — set
        // BEFORE enabling (C2b, Astra round 3); the insertion re-applies both under the lock.
        frame_cryptor.set_key_index(index);
        frame_cryptor.set_enabled(enabled);
        frame_cryptor
    }

    fn setup_rtp_receiver(
        &self,
        participant_identity: &ParticipantIdentity,
        receiver: RtpReceiver,
    ) -> FrameCryptor {
        // TINHAT PATCH (early receiver attach): built WITHOUT holding the manager lock (the early
        // attach runs on libwebrtc's signaling thread and takes this lock too); the callers
        // re-apply the current enabled state under it. Until then the new transformer is
        // disabled, and a disabled receiver drops (discard_frame_when_cryptor_not_ready, U1).
        let (key_provider, enabled) = {
            let inner = self.inner.lock();
            (inner.options.as_ref().unwrap().key_provider.handle.clone(), inner.enabled)
        };

        let frame_cryptor = FrameCryptor::new_for_rtp_receiver(
            LkRuntime::instance().pc_factory(),
            participant_identity.to_string(),
            EncryptionAlgorithm::AesGcm,
            key_provider,
            receiver,
        );
        frame_cryptor.set_enabled(enabled);
        frame_cryptor
    }

    fn remove_frame_cryptor(
        &self,
        participant_identity: ParticipantIdentity,
        track_sid: TrackSid,
        rx_key: Option<usize>,
    ) {
        log::debug!("removing frame cryptor for {}", participant_identity);

        // TINHAT PATCH (early receiver attach): the publication's cryptor + receiver entry go only
        // while that publication still maps to the unsubscribed receiver (unknown receiver: the
        // publication's current one, as upstream); dropped outside the lock (see cleanup)
        let key = (participant_identity, track_sid);
        let removed = {
            let mut inner = self.inner.lock();
            let current = inner.receiver_of.get(&key).copied();
            match (rx_key, current) {
                (Some(k), Some(c)) if k != c => {
                    log::debug!("unsubscribe of {}: its publication maps to a newer receiver, left alone", key.1);
                    // the old receiver's own entry, if it is still subscribed as this publication
                    let entry = match inner.receivers.get(&k) {
                        Some(e) if e.publication.as_ref() == Some(&key) => inner.receivers.remove(&k),
                        _ => None,
                    };
                    (None, entry)
                }
                _ => {
                    let target = rx_key.or(current);
                    if current.is_some() && current == target {
                        inner.receiver_of.remove(&key);
                    }
                    let entry = match target {
                        Some(k) if inner.receivers.get(&k).is_some_and(|e| e.publication.as_ref() == Some(&key)) => {
                            inner.receivers.remove(&k)
                        }
                        _ => None,
                    };
                    (inner.frame_cryptors.remove(&key), entry)
                }
            }
        };
        drop(removed);
    }

    /// Decrypt data received from a data channel
    pub(crate) fn decrypt_data(
        &self,
        data: Vec<u8>,
        iv: Vec<u8>,
        key_index: u32,
        participant_identity: &str,
    ) -> Option<Vec<u8>> {
        let inner = self.inner.lock();

        let data_packet_cryptor = inner.data_packet_cryptor.as_ref()?;

        let encrypted_packet = EncryptedPacket { data, iv, key_index };
        match data_packet_cryptor.decrypt(participant_identity, &encrypted_packet) {
            Ok(decrypted_data) => Some(decrypted_data),
            Err(e) => {
                log::warn!("handle_encrypted_data error: {}", e);
                None
            }
        }
    }

    /// Encrypt data for transmission over a data channel
    pub(crate) fn encrypt_data(
        &self,
        data: Vec<u8>,
        participant_identity: &ParticipantIdentity,
        key_index: u32,
    ) -> Result<EncryptedPacket, Box<dyn std::error::Error>> {
        let inner = self.inner.lock();

        let data_packet_cryptor =
            inner.data_packet_cryptor.as_ref().ok_or("DataPacketCryptor is not initialized")?;

        data_packet_cryptor.encrypt(participant_identity.as_str(), key_index, &data)
    }
}

impl Debug for E2eeManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("E2eeManager")
            .field("enabled", &self.inner.lock().enabled)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receiver_packet_trailer_handler_is_skipped_without_features() {
        assert!(!needs_video_receiver_packet_trailer_handler(&[]));
    }

    #[test]
    fn receiver_packet_trailer_handler_is_needed_for_user_timestamp() {
        let features = [PacketTrailerFeature::PtfUserTimestamp as i32];

        assert!(needs_video_receiver_packet_trailer_handler(&features));
    }

    #[test]
    fn receiver_packet_trailer_handler_is_needed_for_frame_id() {
        let features = [PacketTrailerFeature::PtfFrameId as i32];

        assert!(needs_video_receiver_packet_trailer_handler(&features));
    }
}
