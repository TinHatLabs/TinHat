//! RoomController — open a room, show its recent timeline, send text (M3 chat).
//!
//! v1 loads recent history with `Room::messages` (decrypted by the SDK in E2EE
//! rooms) and renders sender/body/ts as JSON for QML. Sending uses `Room::send`.
//! Also exposes whether the room has an active MatrixRTC call (drives the
//! "Share screen" affordance in the UI). A richer live `matrix-sdk-ui` Timeline
//! (edits/reactions/read-receipts) is a later upgrade.

use core::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use matrix_sdk::room::MessagesOptions;
use matrix_sdk::ruma::events::room::message::RoomMessageEventContent;
use matrix_sdk::ruma::{RoomId, UInt};
use matrix_sdk::Room;

#[cxx_qt::bridge]
pub mod qobject {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name] // snake_case Rust -> camelCase QML (roomId, roomName, messagesJson, callActive)
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(QString, room_id)] //       QML: roomId
        #[qproperty(QString, room_name)] //     QML: roomName
        #[qproperty(QString, messages_json)] // QML: messagesJson — JSON [{sender,body,ts,id}]
        #[qproperty(QString, read_tail)] // QML: readTail — 12.1.17 round 2: a hidden photo newer than every row ("" = none)
        #[qproperty(QString, status)] //        QML: status (errors)
        #[qproperty(QString, op_failed_json)] // QML: opFailedJson — L1: the last refused edits / deletes, newest last (at most 8: push_failure) [{seq, room, kind ("edit" | "delete"), id, text, cls (fail_class)}]; the view handles each new seq once and puts a refused edit back in the box
        #[qproperty(bool, busy)] //             QML: busy
        #[qproperty(bool, call_active)] //      QML: callActive — room has a live call
        #[qproperty(bool, watch_party_on)] //   QML: watchPartyOn — someone is watching a watch party here right now (10.1 TH-WP-04)
        #[qproperty(QString, typing_json)] //  QML: typingJson — JSON [displayName] currently typing
        #[qproperty(bool, loading_older)] //   QML: loadingOlder — older-history fetch in flight
        #[qproperty(bool, has_more)] //        QML: hasMore — older history may still exist
        #[qproperty(QString, members_json)] //      QML: membersJson — JSON [{mxid,name,avatar,power,role,membership,roleName,roleColor,group,groupColor,tagName,tagColor}]
        #[qproperty(QString, room_summary_json)] // QML: roomSummaryJson — {name,topic,alias,encrypted,memberCount,avatar}
        #[qproperty(i32, my_power)] //              QML: myPower — our own power level in the open room
        #[qproperty(bool, can_change_name)] //      QML: canChangeName — may edit m.room.name
        #[qproperty(bool, can_change_topic)] //     QML: canChangeTopic — may edit m.room.topic
        #[qproperty(bool, can_invite)] //           QML: canInvite — may invite users
        #[qproperty(bool, can_kick)] //             QML: canKick — may kick members
        #[qproperty(bool, can_ban)] //              QML: canBan — may ban members
        #[qproperty(bool, can_change_power)] //     QML: canChangePower — may edit m.room.power_levels
        #[qproperty(QString, mod_error)] //         QML: modError — friendly message on a failed moderation write ("" = ok)
        #[qproperty(QString, game_code_json)] //    QML: gameCodeJson — latest game join code {game,code,sender} ("" = none)
        #[qproperty(QString, media_opened)] //      QML: mediaOpened — file:// of the media open_media just resolved
        #[qproperty(QString, media_text)] //        QML: mediaText — {id,name,text} of a text-like attachment open_media shows in the app ("" = none)
        #[qproperty(QString, media_busy)] //        QML: mediaBusy — event id being fetched for Open / Save as ("" = idle); the chip spins off it (10.1 TH-MSG-13)
        #[qproperty(bool, record_on)] //            QML: recordOn — Memories recording marker active in this room (0.8.7)
        #[qproperty(bool, call_record_on)] //       QML: callRecordOn — the recording marker of callRecordRoom (11.0: Rec / Clip act on the call's room)
        #[qproperty(QString, call_record_room)] //  QML: callRecordRoom — the room callRecordOn describes (the call's room; "" = none)
        #[qproperty(QString, memory_photos_json)] // QML: memoryPhotosJson — Memories photos {room, photos:[…], unavailable, iModerate, canAdd, me, encrypted} (memories_photos.rs)
        #[qproperty(QString, memory_photos_notice)] // QML: memoryPhotosNotice — "<n>|<words>" of the last photo action, for a toast
        #[qproperty(QString, memories_json)] //     QML: memoriesJson — DVR sessions {sessions:[…]} from the vods API (0.8.8)
        #[qproperty(QString, clips_json)] //        QML: clipsJson — clip bookmarks [{at,by}] (0.8.8)
        #[qproperty(bool, can_change_photo)] //     QML: canChangePhoto — may WE set the room picture?
        #[qproperty(QString, background_json)] //   QML: backgroundJson — {content, sender} of dev.tinhat.chat.background ("" = absent; docs/AMBIENCE.md)
        #[qproperty(bool, can_set_background)] //  QML: canSetBackground — may WE change the room's background (its power levels)?
        #[qproperty(bool, can_set_background_known)] // QML: canSetBackgroundKnown — false until canSetBackground was read for this room (bgsheet-1220)
        #[qproperty(QString, background_image)] //  QML: backgroundImage — file URL of the background's own image when core imageShown said yes ("" = the preset)
        #[qproperty(QString, background_image_cells)] // QML: backgroundImageCells — its colour cells (every pixel, ambience::image_cells) as JSON
        #[qproperty(QString, media_resolved)] //    QML: mediaResolved — {"id","url"} for INLINE playback (no launch)
        #[qproperty(QString, read_marker_id)] //     QML: readMarkerId — the event my read receipt sat on when the room opened (9.3 unread divider)
        #[qproperty(QString, pinned_json)] //        QML: pinnedJson — [{id,senderName,body,ts, sender,avatar,kind,text,pinIndex}] pinned messages present in the window (9.3; widgets W1 added the last five)
        #[qproperty(i32, pinned_count)] //           QML: pinnedCount — how many events the room has pinned (widgets W1: the Pinned widget's count)
        #[qproperty(bool, can_pin)] //               QML: canPin — may we edit m.room.pinned_events? (9.3)
        #[qproperty(QString, dice_json)] //          QML: diceJson — widgets W3: the loaded window's dice rolls [{id, sender, senderName, avatar, ts, field, pending, failed}] (the Dice widget's room history)
        #[qproperty(bool, can_send_message)] //      QML: canSendMessage — widgets W3: may we send m.room.message here (Dice: roll to the room)
        #[qproperty(QString, polls_json)] //         QML: pollsJson — widgets W3: the window's open polls [{id, ts, question, line}], newest first (the Polls widget)
        #[qproperty(bool, can_send_poll)] //         QML: canSendPoll — widgets W3: may we start a poll here (the poll start event)
        #[qproperty(QString, thread_root)] //        QML: threadRoot — threads (docs/THREADS.md): this controller shows that thread (the Threads panel); "" = the room's main timeline. Set before open().
        #[qproperty(QString, threads_json)] //       QML: threadsJson — threads: the room's threads for the list [{root, rootSender, rootName, rootAvatar, snippet, count, lastName, lastTs, unread, mine}] (loadThreads)
        #[qproperty(bool, can_redact_others)] //     QML: canRedactOthers — A30.0: may we delete other people's messages (the redact level)
        type RoomController = super::RoomControllerRust;

        /// QML: room.linkClaim(formatted, link) — review follow-up (Astra, 2026-10-26): `{"claimed","real"}` when this
        /// link's text in a bubble's sanitised HTML names another host than the one it opens (the bubble asks before
        /// opening it, in Tin Hat Web's words), "" otherwise (richtext::link_claim).
        #[qinvokable]
        #[cxx_name = "linkClaim"]
        fn link_claim(self: &RoomController, formatted: &QString, link: &QString) -> QString;
        /// QML: roomController.setBackground(contentJson) — write the room's shared background (docs/AMBIENCE.md;
        /// the content comes from AmbienceController.content).
        #[qinvokable]
        fn set_background(self: Pin<&mut RoomController>, content_json: &QString);
        /// QML: roomController.postBackgroundImage(fileUrl, preset, effect) — re-encode the image (JPEG, ≤ 2560 px,
        /// ≤ 1.5 MB, no metadata), post it to the chat as a normal image message (encrypted in an encrypted room), then
        /// set it as the room's background with `preset` as the fallback.
        #[qinvokable]
        fn post_background_image(self: Pin<&mut RoomController>, file_url: &QString, preset: &QString, effect: &QString);
        /// QML: roomController.open(roomId) — load a room's timeline.
        #[qinvokable]
        fn open(self: Pin<&mut RoomController>, room_id: &QString);
        /// QML: roomController.localFileSize(url) — QOL A08.7: a local file's size in bytes for the staged chip
        /// (0 = unknown).
        #[qinvokable]
        #[cxx_name = "localFileSize"]
        fn local_file_size(self: &RoomController, url: &QString) -> i64;
        /// QML: roomController.refresh() — reload the current room's timeline.
        #[qinvokable]
        fn refresh(self: Pin<&mut RoomController>);
        /// QML: roomController.send(text) — send a plain-text message.
        #[qinvokable]
        fn send(self: Pin<&mut RoomController>, text: &QString);
        /// QML: roomController.sendWithMentions(text, mentionsJson) — send with @mentions.
        /// mentionsJson = [{name,userId}]; produces an HTML body with matrix.to "pill"
        /// links + an `m.mentions` user_ids list (the actual ping). Falls back to a plain
        /// send if no mentions resolve.
        #[qinvokable]
        fn send_with_mentions(self: Pin<&mut RoomController>, text: &QString, mentions_json: &QString);
        /// QML: roomController.setGameCode(game, code) — post the current game's join code as
        /// a custom `dev.tinhat.game.code` timeline event (any member can; latest wins). An
        /// empty `code` clears it. Shown in the dedicated join-code bar, never as a chat bubble.
        #[qinvokable]
        fn set_game_code(self: Pin<&mut RoomController>, game: &QString, code: &QString);
        /// QML: roomController.react(eventId, key) — add an emoji reaction.
        #[qinvokable]
        fn react(self: Pin<&mut RoomController>, event_id: &QString, key: &QString);
        /// QML: roomController.unreact(reactionId) — redact our own reaction event.
        #[qinvokable]
        fn unreact(self: Pin<&mut RoomController>, reaction_id: &QString);
        /// QML: roomController.sendReply(parentId, text) — reply to a message.
        #[qinvokable]
        fn send_reply(self: Pin<&mut RoomController>, parent_id: &QString, text: &QString);
        /// QML: roomController.sendReplyMentions(parentId, text, mentionsJson) — 10.6 (REACH A5.2):
        /// a reply that keeps the @-mentions picked in the composer ([{name,userId}], their links
        /// included). Every reply mentions the parent's sender, so they are notified.
        #[qinvokable]
        fn send_reply_mentions(self: Pin<&mut RoomController>, parent_id: &QString, text: &QString, mentions_json: &QString);
        /// QML: roomController.editMessage(eventId, text) — edit our message (m.replace).
        #[qinvokable]
        fn edit_message(self: Pin<&mut RoomController>, event_id: &QString, text: &QString);
        /// QML: roomController.editMessageMentions(eventId, text, mentionsJson) — L1 A09.3: an edit with the people
        /// picked while editing ([{name,userId}]); the original's own mention links are kept either way (edit_mentions).
        #[qinvokable]
        fn edit_message_mentions(self: Pin<&mut RoomController>, event_id: &QString, text: &QString, mentions_json: &QString);
        /// QML: roomController.redactMessage(eventId) — delete/redact a message.
        #[qinvokable]
        fn redact_message(self: Pin<&mut RoomController>, event_id: &QString);
        /// 9.3: `/me` — an m.emote ("* Blake waves"), through the same local-echo path as send().
        #[qinvokable]
        fn send_emote(self: Pin<&mut RoomController>, text: &QString);
        /// QML: roomController.sendDice(contentJson) — widgets W3: a dice roll (DiceController.rollForRoom's content: the
        /// shared m.notice + HTML + `dev.tinhat.dice`) through the local-echo path; refused unless it reads as a roll.
        #[qinvokable]
        fn send_dice(self: Pin<&mut RoomController>, content_json: &QString);
        /// QML: roomController.sendPoll(question, answersJson, disclosed) — widgets W3: a new poll (the shared startContent,
        /// answer ids from the OS CSPRNG) through the local-echo path; → "" (sent) or the friendly error.
        #[qinvokable]
        fn send_poll(self: Pin<&mut RoomController>, question: &QString, answers_json: &QString, disclosed: bool) -> QString;
        /// QML: roomController.votePoll(startId, answersJson) — widgets W3: a vote (the card's `next`); the card changes at once.
        #[qinvokable]
        fn vote_poll(self: Pin<&mut RoomController>, start: &QString, answers_json: &QString);
        /// QML: roomController.endPoll(startId) — widgets W3: end a poll (its creator or a moderator); the end message is the
        /// shared endText of the tally so far.
        #[qinvokable]
        fn end_poll(self: Pin<&mut RoomController>, start: &QString);
        /// 9.3: resend a message whose local echo failed (`txn` = that row's txn).
        #[qinvokable]
        fn retry_send(self: Pin<&mut RoomController>, txn: &QString);
        /// 9.3: drop a failed local echo without sending it.
        #[qinvokable]
        fn discard_echo(self: Pin<&mut RoomController>, txn: &QString);
        /// 9.3: pin / unpin a message (m.room.pinned_events).
        #[qinvokable]
        fn pin_message(self: Pin<&mut RoomController>, event_id: &QString, pinned: bool);
        /// QML: roomController.setTyping(bool) — send our typing notice (debounced in QML).
        #[qinvokable]
        fn set_typing(self: Pin<&mut RoomController>, typing: bool);
        /// QML: roomController.loadOlder() — grow the loaded window + prepend older history.
        #[qinvokable]
        fn load_older(self: Pin<&mut RoomController>);
        /// QML: roomController.trimToLatest() — A35.4: the reader is back at the newest (Jump to latest / End): the
        /// window goes back to the default depth, so the live loop stops reloading a deep scroll-back on every pass.
        #[qinvokable]
        fn trim_to_latest(self: Pin<&mut RoomController>);
        /// QML: roomController.sendAttachment(localPathOrFileUrl) — upload + send a
        /// file/image (E2EE-safe via the SDK's encrypting send-queue helper).
        #[qinvokable]
        fn send_attachment(self: Pin<&mut RoomController>, local_path: &QString);
        /// QML: room.sendVoiceNote(requestJson) — voice notes (docs/VOICE-NOTES.md §1): the draft's
        /// {room, path, durationMs, waveform, size, txn} (VoiceNoteController.takeSendRequest). One local echo (m.audio,
        /// playable from the local file); upload (encrypted in an E2EE room: `file`, never `url`) → voice_content →
        /// send_raw with the draft's transaction id. A retry passes the same txn: the same row, the finished upload reused.
        #[qinvokable]
        fn send_voice_note(self: Pin<&mut RoomController>, request_json: &QString);
        /// QML: room.clipboardAttachments() — JSON ["file://…"] of what a Ctrl+V should attach
        /// instead of typing: an image on the clipboard (a screenshot, a browser's "Copy image")
        /// saved as a PNG in the media cache, or files copied in a file manager. "[]" when the
        /// clipboard holds only text — the composer pastes that itself (9.3.2).
        #[qinvokable]
        #[cxx_name = "clipboardAttachments"]
        fn clipboard_attachments(self: &RoomController) -> QString;
        /// QML: roomController.openMedia(eventId) — download+decrypt that event's
        /// video/file/audio to the media cache and surface it via `mediaOpened`
        /// (QML opens the file:// externally). Images use the inline path instead.
        #[qinvokable]
        fn open_media(self: Pin<&mut RoomController>, event_id: &QString);
        /// QML: room.saveMediaAs(eventId, destUrl) — download/decrypt (or reuse the
        /// cache) and copy the file where the user asked. Keeping the copy in Rust means
        /// the encrypted-media case is handled once, in the place that already knows how.
        #[qinvokable]
        #[cxx_name = "saveMediaAs"]
        fn save_media_as(self: Pin<&mut RoomController>, event_id: &QString, dest: &QString);
        /// QML: room.resolveMedia(eventId) — download/decrypt (or reuse the cache) and
        /// publish the local url on `mediaResolved` WITHOUT launching anything, so a
        /// video can play inside the bubble. Separate from openMedia precisely because
        /// that one's whole job is to hand the file to another program.
        #[qinvokable]
        #[cxx_name = "resolveMedia"]
        fn resolve_media(self: Pin<&mut RoomController>, event_id: &QString);
        /// QML: roomController.markRead() — send our read receipt for the newest loaded
        /// message. Called only when that message is actually visible at the bottom of a
        /// focused timeline (NOT from the sync loop), to avoid a receipt-echo storm.
        #[qinvokable]
        fn mark_read(self: Pin<&mut RoomController>);
        /// QML: room.loadThreads() — threads: the room's threads for the Threads list (the server's /threads + the
        /// thread replies the window holds) into threadsJson.
        #[qinvokable]
        fn load_threads(self: Pin<&mut RoomController>);
        /// QML: room.loadMoreThreads() — threads: one more page of the server's /threads (the list's "Load more threads").
        #[qinvokable]
        fn load_more_threads(self: Pin<&mut RoomController>);
        /// QML: room.threadSummaryText(count, name, ts) — threads: "12 replies · Ana · 5 min ago" (the shared core's
        /// words, now = this call's clock; the web prints the same bytes).
        #[qinvokable]
        fn thread_summary_text(self: &RoomController, count: i32, name: &QString, ts: f64) -> QString;
        /// QML: room.threadSummaryLabel(count, name, ts) — the same for screen readers ("…, last from Ana 5 min ago.
        /// Open thread").
        #[qinvokable]
        fn thread_summary_label(self: &RoomController, count: i32, name: &QString, ts: f64) -> QString;
        /// QML: room.threadOfEvent(eventId) — threads: the thread a loaded event is in ("" = main / not loaded): a
        /// jump to a thread reply opens its thread (thread replies never show in the main timeline).
        #[qinvokable]
        fn thread_of_event(self: &RoomController, event_id: &QString) -> QString;
        /// QML: roomController.applyRoomName(name) — rename the room (m.room.name).
        /// Named apply_room_name (NOT set_room_name) — a set_room_name is already
        /// auto-generated by the room_name qproperty.
        #[qinvokable]
        fn apply_room_name(self: Pin<&mut RoomController>, name: &QString);
        /// QML: roomController.applyRoomTopic(topic) — set the room topic (m.room.topic).
        #[qinvokable]
        fn apply_room_topic(self: Pin<&mut RoomController>, topic: &QString);
        /// QML: room.applyRoomPhoto(fileUrl) — upload an image and make it the room's
        /// picture. Pass "" to remove it. Works for spaces too; a space is a room.
        #[qinvokable]
        #[cxx_name = "applyRoomPhoto"]
        fn apply_room_photo(self: Pin<&mut RoomController>, path: &QString);
        /// QML: room.setRecordCall(on) — Memories call recording on/off (0.8.7). ON
        /// invites the recorder (idempotent) and writes the marker; recording actually
        /// starts when the bot joins the call — visibly, as a participant.
        #[qinvokable]
        #[cxx_name = "setRecordCall"]
        fn set_record_call(self: Pin<&mut RoomController>, on: bool);
        /// QML: room.clipMoment() — drop a clip bookmark at NOW (dev.tinhat.clip
        /// state; shows as a marker on the Memories timeline). Any member may clip.
        #[qinvokable]
        #[cxx_name = "clipMoment"]
        fn clip_moment(self: Pin<&mut RoomController>);
        /// QML: room.setRecordCallIn(roomId, on) — setRecordCall for any joined room: the call status line's Rec acts
        /// on the CALL's room, also while another room is on screen (11.0, a deliberate choice 2026-09-27; the web did
        /// already). recordOn (the open room) and callRecordOn (callRecordRoom) follow when they name that room.
        #[qinvokable]
        #[cxx_name = "setRecordCallIn"]
        fn set_record_call_in(self: Pin<&mut RoomController>, room_id: &QString, on: bool);
        /// QML: room.clipMomentIn(roomId) — clipMoment for any joined room (the call's room).
        #[qinvokable]
        #[cxx_name = "clipMomentIn"]
        fn clip_moment_in(self: Pin<&mut RoomController>, room_id: &QString);
        /// QML: room.refreshCallRecord(roomId) — read that room's recording marker into callRecordOn (callRecordRoom =
        /// roomId; "" clears both). Called when the call's room changes (ActionsHost.qml).
        #[qinvokable]
        #[cxx_name = "refreshCallRecord"]
        fn refresh_call_record(self: Pin<&mut RoomController>, room_id: &QString);
        /// Memories LIVE side tiles: create / attach / clock-sync / retire
        /// one muted gst playback tile (streamer::memtile). All GUI-thread (the
        /// qml6glsink affinity rule); seconds cross the QML boundary as STRINGS
        /// (project rule — raw doubles across the bridge have bitten us before).
        #[qinvokable]
        #[cxx_name = "memtileCreate"]
        fn memtile_create(self: Pin<&mut RoomController>, id: &QString, url: &QString) -> bool;
        #[qinvokable]
        #[cxx_name = "memtileAttach"]
        fn memtile_attach(self: Pin<&mut RoomController>, id: &QString) -> bool;
        #[qinvokable]
        #[cxx_name = "memtileSync"]
        fn memtile_sync(self: Pin<&mut RoomController>, id: &QString, secs: &QString, hard: bool);
        #[qinvokable]
        #[cxx_name = "memtileSetPaused"]
        fn memtile_set_paused(self: Pin<&mut RoomController>, id: &QString, paused: bool);
        #[qinvokable]
        #[cxx_name = "memtileDestroy"]
        fn memtile_destroy(self: Pin<&mut RoomController>, id: &QString);
        #[qinvokable]
        #[cxx_name = "memtileClear"]
        fn memtile_clear(self: Pin<&mut RoomController>);
        /// Has this tile's pipeline died at runtime (missing decoder, TLS, 404)?
        /// QML flips a dead tile to the thumbnail fallback.
        #[qinvokable]
        #[cxx_name = "memtileDead"]
        fn memtile_dead(self: Pin<&mut RoomController>, id: &QString) -> bool;
        /// Buffers rendered by this tile so far (-1 = no tile). Health-poll signal:
        /// zero frames after the post-attach grace period = dead pipeline, however
        /// it got there — the decoder-less wedge posts NO bus error.
        #[qinvokable]
        #[cxx_name = "memtileFrames"]
        fn memtile_frames(self: Pin<&mut RoomController>, id: &QString) -> i32;
        /// Tile position in seconds of ITS OWN file as a string ("-1" = can't answer
        /// yet / no tile). 0.8.16: lets the QML settle a tile only once it is
        /// actually playing near the master clock (no more file-start flash).
        #[qinvokable]
        #[cxx_name = "memtilePosition"]
        fn memtile_position(self: Pin<&mut RoomController>, id: &QString) -> QString;
        /// Tri-state H.264 real-decode probe verdict (0 = still probing, 1 = ok,
        /// 2 = broken). Polling this from QML also KICKS the async test decode
        /// (streamer::memtile::kick_probe), so the verdict is usually in before the
        /// first tile create; 2 = skip gst tiles, thumbnails directly.
        #[qinvokable]
        #[cxx_name = "memtileProbeState"]
        fn memtile_probe_state(self: Pin<&mut RoomController>) -> i32;
        /// QML: room.fetchMemories() — load this room's recording sessions from the
        /// server's vods API into memoriesJson (bearer = our own access token).
        #[qinvokable]
        #[cxx_name = "fetchMemories"]
        fn fetch_memories(self: Pin<&mut RoomController>);
        /// QML: fetchMemoriesFor(roomId) — 10.1 (TH-MEM-09): the Memories window keeps ITS room.
        #[qinvokable]
        #[cxx_name = "fetchMemoriesFor"]
        fn fetch_memories_for(self: Pin<&mut RoomController>, room_id: &QString);
        /// QML: room.fetchClips() — mirror clip bookmarks into clipsJson.
        #[qinvokable]
        #[cxx_name = "fetchClips"]
        fn fetch_clips(self: Pin<&mut RoomController>);
        /// QML: fetchClipsFor(roomId) — 10.1 (TH-MEM-09)
        #[qinvokable]
        #[cxx_name = "fetchClipsFor"]
        fn fetch_clips_for(self: Pin<&mut RoomController>, room_id: &QString);
        /// QML: room.fetchMemoryPhotos(roomId) — that room's Memories photos into memoryPhotosJson (memories_photos.rs).
        #[qinvokable]
        #[cxx_name = "fetchMemoryPhotos"]
        fn fetch_memory_photos(self: Pin<&mut RoomController>, room_id: &QString);
        /// QML: room.addToMemories(roomId, eventId, ts) — one of MY image messages onto my Memories list.
        #[qinvokable]
        #[cxx_name = "addToMemories"]
        fn add_to_memories(self: Pin<&mut RoomController>, room_id: &QString, event_id: &QString, ts: f64);
        /// QML: room.removeFromMemories(roomId, eventId, alsoDelete) — off my list (the message stays unless alsoDelete).
        #[qinvokable]
        #[cxx_name = "removeFromMemories"]
        fn remove_from_memories(self: Pin<&mut RoomController>, room_id: &QString, event_id: &QString, also_delete: bool);
        /// QML: room.takeDownMemory(roomId, eventId) — a moderator's take-down of someone else's photo.
        #[qinvokable]
        #[cxx_name = "takeDownMemory"]
        fn take_down_memory(self: Pin<&mut RoomController>, room_id: &QString, event_id: &QString);
        /// QML: room.deleteMemoryPhoto(roomId, eventId) — 12.1.17: a moderator's Delete in the photo viewer (redacts the
        /// message: gone from the chat and from Memories; the photo isn't in the chat, so its delete lives there too).
        #[qinvokable]
        #[cxx_name = "deleteMemoryPhoto"]
        fn delete_memory_photo(self: Pin<&mut RoomController>, room_id: &QString, event_id: &QString);
        /// QML: room.addMemoryPhotos(roomId, urlsJson) — "Add photos": each image file sent to that room's chat and
        /// added to my Memories (["file://…"], from the picker, a paste or a drop).
        #[qinvokable]
        #[cxx_name = "addMemoryPhotos"]
        fn add_memory_photos(self: Pin<&mut RoomController>, room_id: &QString, urls_json: &QString);
        /// QML: room.inMyMemories(roomId, eventId) — the chat menu's Add / Remove label.
        #[qinvokable]
        #[cxx_name = "inMyMemories"]
        fn in_my_memories(self: &RoomController, room_id: &QString, event_id: &QString) -> bool;
        /// QML: room.hiddenInChat(roomId, eventId) — 12.1.17: is this a photo the chat hid at its last load (it is in its
        /// sender's Memories)? A jump to it opens the Memories viewer instead of hunting the history.
        #[qinvokable]
        #[cxx_name = "hiddenInChat"]
        fn hidden_in_chat(self: &RoomController, room_id: &QString, event_id: &QString) -> bool;
        /// QML: room.memoriesTimeline(sessionsJson, photosJson, filter) — the shared timeline (memphotos::timeline).
        #[qinvokable]
        #[cxx_name = "memoriesTimeline"]
        fn memories_timeline(self: &RoomController, sessions_json: &QString, photos_json: &QString, filter: &QString) -> QString;
        /// QML: room.memoriesStep(itemsJson, current, dir) — the photo viewer's Previous / Next (memphotos::step).
        #[qinvokable]
        #[cxx_name = "memoriesStep"]
        fn memories_step(self: &RoomController, items_json: &QString, current: &QString, dir: i32) -> QString;
        /// QML: room.saveMemoryPhoto(mediaUrl, destUrl) — Save: copy the photo's decrypted copy (in the media cache) where
        /// the user asked (the Memories window's room needn't be the open one, so saveMediaAs can't serve it).
        #[qinvokable]
        #[cxx_name = "saveMemoryPhoto"]
        fn save_memory_photo(self: Pin<&mut RoomController>, media_url: &QString, dest: &QString);
        /// QML: room.memoryPhotoExts() — the file types "Add photos" takes, as a JSON list of extensions (12.1.17:
        /// memories_photos::PHOTO_EXTS, the web's image/* minus SVG: phone photos included).
        #[qinvokable]
        #[cxx_name = "memoryPhotoExts"]
        fn memory_photo_exts(self: &RoomController) -> QString;
        /// QML: room.memoryPhotosGate() — the gate hook TINHAT_GUI_MEMPHOTOS (file urls to add once; "" in every real
        /// launch). MemoriesWindow.qml logs [memphotos] lines while it is set.
        #[qinvokable]
        #[cxx_name = "memoryPhotosGate"]
        fn memory_photos_gate(self: &RoomController) -> QString;
        /// QML: room.memoriesWindowMode(recorder) — the Memories window on a server with / without a call recorder
        /// (memphotos::window_mode as JSON: recordings, filters, the empty / subtitle / tip words).
        #[qinvokable]
        #[cxx_name = "memoriesWindowMode"]
        fn memories_window_mode(self: &RoomController, recorder: bool) -> QString;
        /// QML: room.memoryPhotosRev(roomId) — how many Memories changes the sync brought for that room (2-3: an add,
        /// a remove, a take-down, a redacted photo); the open window re-reads when it moves.
        #[qinvokable]
        #[cxx_name = "memoryPhotosRev"]
        fn memory_photos_rev(self: &RoomController, room_id: &QString) -> i32;
        /// QML: room.memoriesCountText(recordings, photos) — "3 recordings · 12 photos".
        #[qinvokable]
        #[cxx_name = "memoriesCountText"]
        fn memories_count_text(self: &RoomController, recordings: i32, photos: i32) -> QString;

        /// QML: roomController.inviteUser(userId) — invite a user by mxid.
        #[qinvokable]
        fn invite_user(self: Pin<&mut RoomController>, user_id: &QString);
        /// QML: roomController.kickUser(userId, reason) — remove a member (re-joinable).
        #[qinvokable]
        fn kick_user(self: Pin<&mut RoomController>, user_id: &QString, reason: &QString);
        /// QML: roomController.banUser(userId, reason) — ban a member.
        #[qinvokable]
        fn ban_user(self: Pin<&mut RoomController>, user_id: &QString, reason: &QString);
        /// QML: roomController.unbanUser(userId, reason) — lift a ban.
        #[qinvokable]
        fn unban_user(self: Pin<&mut RoomController>, user_id: &QString, reason: &QString);
        /// QML: roomController.setMemberPower(userId, power) — set a member's power level.
        #[qinvokable]
        fn set_member_power(self: Pin<&mut RoomController>, user_id: &QString, power: i32);
        /// QML: roomController.setAttending(on) — 10.1 (TL-03): this view of the room is on screen
        /// in a focused window. Drives ONLY the notifier's "they can already see it" suppression;
        /// receipt-free (read receipts are gated in QML on the same flag).
        #[qinvokable]
        #[cxx_name = "setAttending"]
        fn set_attending(self: Pin<&mut RoomController>, on: bool);
        /// QML: roomController.close() — abort this controller's live sync + typing tasks.
        /// Called from a floating bubble's Window.onClosing BEFORE the QObject is destroyed
        /// (dropping a JoinHandle only DETACHES the task — it does NOT abort it, so without
        /// this an orphaned subscribe/typing loop would leak for the rest of the session).
        /// Receipt-safe: aborts background loops only; sends nothing.
        #[qinvokable]
        fn close(self: Pin<&mut RoomController>);
    }

    impl cxx_qt::Threading for RoomController {}
}

#[derive(Default)]
pub struct RoomControllerRust {
    // 10.1: identity of this view in the notifier's attended-rooms table (0 = not assigned yet).
    attend_token: u64,
    // 10.5.2 (timeline): bumped by every open(). A background result is applied only while it still
    // belongs to the SAME open of the SAME room — the room-id stamp alone let a load started on an
    // earlier visit land after A → B → A.
    open_gen: u64,
    room_id: QString,
    room_name: QString,
    messages_json: QString,
    read_tail: QString,
    status: QString,
    op_failed_json: QString,
    busy: bool,
    call_active: bool,
    watch_party_on: bool,
    typing_json: QString,
    loading_older: bool,
    has_more: bool,
    members_json: QString,
    room_summary_json: QString,
    my_power: i32,
    can_change_name: bool,
    can_change_topic: bool,
    can_invite: bool,
    can_kick: bool,
    can_ban: bool,
    can_change_power: bool,
    mod_error: QString,
    game_code_json: QString,
    media_opened: QString,
    media_text: QString,
    media_busy: QString,
    media_resolved: QString,
    can_change_photo: bool,
    background_json: QString,
    can_set_background: bool,
    can_set_background_known: bool,
    background_image: QString,
    background_image_cells: QString,
    record_on: bool,
    call_record_on: bool,
    call_record_room: QString,
    memories_json: QString,
    memory_photos_json: QString,
    memory_photos_notice: QString,
    clips_json: QString,
    read_marker_id: QString,
    pinned_json: QString,
    pinned_count: i32,
    can_pin: bool,
    dice_json: QString,
    can_send_message: bool,
    polls_json: QString,
    can_send_poll: bool,
    thread_root: QString,
    threads_json: QString,
    // threads: the newest load_threads request of this controller (round 2 #13)
    threads_seq: u64,
    // threads: how many /threads pages the list shows (Load more threads adds one; a new room starts at 1)
    threads_pages: u32,
    can_redact_others: bool,
    // widgets W3: the window's poll responses and ends ({id, type, sender, ts, content}; my local ones appended until the
    // real ones arrive), who besides a creator may end a poll (the ends' senders who may redact), and whether I may
    poll_rels: Vec<serde_json::Value>,
    poll_enders: Vec<String>,
    poll_can_redact: bool,
    // 9.3: our own display name + avatar in the open room, so a local echo row can be
    // painted without a network round trip (filled by each timeline load).
    own_name: String,
    own_avatar: String,
    // Live subscription to the open room (re-renders on each update); aborted when
    // a different room is opened.
    sub_task: Option<tokio::task::JoinHandle<()>>,
    // Typing-notification subscription for the open room (aborted on room switch).
    typing_task: Option<tokio::task::JoinHandle<()>>,
    // How many messages the timeline window currently wants (grows via load_older).
    // Shared with the live-subscription task so it re-renders at the current depth.
    loaded_limit: Arc<AtomicUsize>,
    // A35.4: Jump to latest came while an older page was loading — shrink the window once it has landed.
    trim_pending: bool,
}

impl qobject::RoomController {
    pub fn link_claim(&self, formatted: &QString, link: &QString) -> QString {
        match crate::richtext::link_claim(&formatted.to_string(), &link.to_string()) {
            Some((claimed, real)) => QString::from(&serde_json::json!({ "claimed": claimed, "real": real }).to_string()),
            None => QString::default(),
        }
    }

    pub fn open(mut self: Pin<&mut Self>, room_id: &QString) {
        let room_id = room_id.to_string();
        // threads: a thread's panel (threadRoot set) shows that thread; "" = the room's main timeline
        let view = self.thread_root().to_string();
        // 10.5.2: a new generation BEFORE anything can observe the new room id.
        let gen = self.rust().open_gen.wrapping_add(1);
        self.as_mut().rust_mut().open_gen = gen;
        self.as_mut().set_room_id(QString::from(&room_id));
        // If this view is currently attended (on screen, focused), the notifier follows it to
        // the new room; otherwise QML's setAttending says when it becomes visible. Receipt-free.
        let token = self.as_mut().attend_token();
        // threads: the room and which thread this view shows (review #15), together (1-3: what comes on screen is that
        // thread, so only its mentions are seen)
        crate::notify::update_view(token, &room_id, &view);
        // May we set this room's picture? Async (power_levels() awaits), so it is
        // computed here and published into  for the menu to bind to —
        // offering a button that is guaranteed to 403 is worse than not offering it.
        self.as_mut().set_can_change_photo(false);
        {
            let rid = room_id.clone();
            let qt = self.as_mut().qt_thread();
            crate::bridge::runtime().spawn(async move {
                let allowed = async {
                    let client = crate::session::client()?;
                    let id = matrix_sdk::ruma::RoomId::parse(&rid).ok()?;
                    let room = client.get_room(&id)?;
                    let pl = room.power_levels().await.ok()?;
                    Some(pl.user_can_send_state(
                        room.own_user_id(),
                        matrix_sdk::ruma::events::StateEventType::RoomAvatar,
                    ))
                }
                .await
                .unwrap_or(false);
                let _ = qt.queue(move |mut o| {
                    if is_current(&o, gen, &rid) {
                        o.as_mut().set_can_change_photo(allowed)
                    }
                });
            });
        }
        // Recording marker (0.8.7): mirror dev.tinhat.record so the room menu
        // offers Stop vs Start. Refreshed at open(); the live
        // in-call cue is the bot's own participant tile (root.recActive in QML).
        self.as_mut().set_record_on(false);
        self.as_mut().set_background_json(QString::default());
        self.as_mut().set_can_set_background(false);
        // bgsheet-1220: not known until the first state read below (the sheet is neutral meanwhile, never read-only)
        self.as_mut().set_can_set_background_known(false);
        self.as_mut().set_background_image(QString::default());
        self.as_mut().set_background_image_cells(QString::default());
        {
            let rid = room_id.clone();
            let qt = self.as_mut().qt_thread();
            crate::bridge::runtime().spawn(async move {
                let on = async {
                    let client = crate::session::client()?;
                    let id = matrix_sdk::ruma::RoomId::parse(&rid).ok()?;
                    let room = client.get_room(&id)?;
                    crate::appstate::record_enabled(&room).await
                }
                .await
                .is_some();
                let _ = qt.queue(move |mut o| {
                    if is_current(&o, gen, &rid) {
                        o.as_mut().set_record_on(on)
                    }
                });
            });
        }
        self.as_mut().set_room_name(QString::default());
        self.as_mut().set_read_marker_id(QString::default());
        self.as_mut().set_pinned_json(QString::default());
        self.as_mut().set_pinned_count(0);
        self.as_mut().set_dice_json(QString::default());
        self.as_mut().set_polls_json(QString::default());
        self.as_mut().rust_mut().poll_rels.clear();
        self.as_mut().set_threads_json(QString::default()); // threads: the list belongs to the room it was loaded for
        self.as_mut().rust_mut().threads_pages = 1;
        // 9.3: paint the on-disk timeline cache SYNCHRONOUSLY, in the write that used
        // to blank the timeline. The old order (blank -> queued cache paint -> live load) let
        // the QML mirror observe the blank between two polls, so every room switch flashed
        // "No messages", rebuilt to the cache, then rebuilt again to the live load: three full
        // delegate rebuilds and a visible flicker. Now the first thing the mirror can see for
        // the new room is its cache (or an empty list while busy, which draws the spinner).
        // 10.5.2: only a snapshot that parses is painted (read_timeline_cache validates it).
        let snapshot = if view.is_empty() { read_timeline_cache(&room_id) } else { None };
        eprintln!(
            "[room] open {room_id} gen={gen} snapshot={}",
            snapshot.as_ref().map_or_else(|| "none".to_string(), |s| format!("{} rows", s.1))
        );
        match snapshot {
            Some((cached, _)) => self.as_mut().set_messages_json(QString::from(&cached)),
            None => self.as_mut().set_messages_json(QString::default()),
        }
        self.as_mut().set_status(QString::default());
        self.as_mut().set_typing_json(QString::default());
        self.as_mut().set_members_json(QString::default());
        self.as_mut().set_room_summary_json(QString::default());
        self.as_mut().set_has_more(false);
        self.as_mut().set_loading_older(false);
        self.as_mut().set_busy(true);
        // Reset the timeline window to the default depth for the newly-opened room.
        let limit = self.as_mut().rust().loaded_limit.clone();
        limit.store(DEFAULT_WINDOW, Ordering::Relaxed);
        self.as_mut().rust_mut().trim_pending = false;

        // Cancel the previous room's live + typing subscriptions.
        if let Some(t) = self.as_mut().rust_mut().sub_task.take() {
            t.abort();
        }
        if let Some(t) = self.as_mut().rust_mut().typing_task.take() {
            t.abort();
        }

        // Instant paint of the cached member list + room summary, so the right panel +
        // header subtitle aren't blank for the seconds the live member sync takes.
        if let Some((m, s)) = read_members_cache(&room_id) {
            let started = room_id.clone();
            let _ = self.qt_thread().queue(move |mut o| {
                if is_current(&o, gen, &started) && o.members_json().is_empty() {
                    o.as_mut().set_members_json(QString::from(&m));
                    o.as_mut().set_room_summary_json(QString::from(&s));
                }
            });
        }

        // ---- Typing-notification subscription (separate task; aborted on switch). ----
        let qt_typing = self.qt_thread();
        let typing_room_id = room_id.clone();
        let typing_task = crate::bridge::runtime().spawn(async move {
            let Some(client) = crate::session::client() else { return };
            let Ok(rid) = RoomId::parse(&typing_room_id) else { return };
            let Some(room) = client.get_room(&rid) else { return };
            // _guard must stay alive for the whole loop (dropping it removes the handler).
            let (_guard, mut rx) = room.subscribe_to_typing_notifications();
            loop {
                match rx.recv().await {
                    Ok(user_ids) => {
                        let mut names: Vec<String> = Vec::with_capacity(user_ids.len());
                        for uid in &user_ids {
                            names.push(resolve_member_name(&room, uid.as_str()).await);
                        }
                        if !names.is_empty() {
                            eprintln!("[room] typing: {names:?}");
                        }
                        let json = serde_json::to_string(&names).unwrap_or_else(|_| "[]".into());
                        let started = typing_room_id.clone();
                        let _ = qt_typing.queue(move |mut o| {
                            if is_current(&o, gen, &started) {
                                o.as_mut().set_typing_json(QString::from(&json));
                            }
                        });
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
        });
        self.as_mut().rust_mut().typing_task = Some(typing_task);

        // ---- Live timeline subscription. ----
        // 10.5.2 (timeline): the timeline paints FIRST. It used to wait behind the read marker, a
        // full key-backup download for the room (on EVERY open — one real account's backup held 600 sessions
        // for a single busy room), the admin power-level provisioning and the member list; until all of
        // that finished, a room you went back into showed only its on-disk snapshot (or a spinner),
        // so what arrived while you were away "didn't load". Now: subscribe, read marker (local
        // store), timeline -> screen, then members, then keys (+ re-decrypt), then provisioning.
        // Every result carries this open's generation (`is_current`).
        let qt = self.qt_thread();
        let limit2 = limit.clone();
        let task = crate::bridge::runtime().spawn(async move {
            let opened = std::time::Instant::now();
            let Some(client) = crate::session::client() else { return };
            let Ok(rid) = RoomId::parse(&room_id) else { return };
            // A room joined a moment ago can be one sync away from the local store: wait for it
            // briefly instead of leaving the spinner up for good (`busy` was never cleared).
            // (12.1.16) a link to a room this account isn't in: keep waiting while the first sync isn't done
            // (room_missing, the web's rule; the task is aborted when another room opens)
            let mut found = client.get_room(&rid);
            let mut waited: u64 = 0;
            while found.is_none() && !room_missing(false, ROOM_SYNCED.load(Ordering::Relaxed), waited) {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                waited += 250;
                found = client.get_room(&rid);
            }
            // 12.1.17 (review): the first sync can land between that last look and the flag it sets; look once more, or a
            // room the sync just brought is called missing for the second before the reopen below
            let found = found.or_else(|| client.get_room(&rid));
            let Some(room) = found else {
                eprintln!("[room] {room_id} gen={gen}: not in the local store after 10 s — nothing to load");
                let started = room_id.clone();
                let _ = qt.queue(move |mut o| {
                    if is_current(&o, gen, &started) {
                        o.as_mut().set_busy(false);
                        // a link to a room this account isn't in: RoomMissing.qml shows the way back on these words
                        o.as_mut().set_status(QString::from(ROOM_MISSING_EXPLANATION));
                    }
                });
                // a room that arrives later (a slow sync, a join on another device) still opens
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    if client.get_room(&rid).is_some() {
                        let again = room_id.clone();
                        let _ = qt.queue(move |mut o| {
                            if is_current(&o, gen, &again) {
                                o.as_mut().open(&QString::from(&again));
                            }
                        });
                        return;
                    }
                }
            };
            watch_room_keys(&client);
            // Subscribe BEFORE the first load, so an update that lands in between is not missed.
            let mut rx = room.subscribe_to_updates();
            let mut keys_rx = keys_channel().subscribe();
            // 9.3: where my read receipt sits, published before anything slow. The unread divider
            // is drawn after this event; QML freezes the value per room open so our own later
            // receipts never move the line while the room is on screen.
            // threads: a thread's panel has no NEW divider (the main receipt doesn't describe it)
            if view.is_empty() {
                // public or private: a room on another server gets private receipts (receipts.rs)
                // boxed: the event fetch inside is a deep future (rustc's query depth limit in this task)
                let fut: std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send + '_>> = Box::pin(crate::receipts::own_read_up_to(&room));
                let marker = fut.await;
                let started = room_id.clone();
                let _ = qt.queue(move |mut o| {
                    if is_current(&o, gen, &started) {
                        o.as_mut().set_read_marker_id(QString::from(&marker));
                    }
                });
            }
            // 9.3: the member list + our permissions are re-derived only when a sync batch
            // carried STATE (a join, a rename, a power-level change) — not per message. The
            // old loop walked every member and resolved every avatar on each incoming line.
            let mut state_changed = true;
            // chat backgrounds: the state last read, and when its image was last resolved (None = resolve now). The image
            // is re-checked every 20 s while it is an image background: a deleted image goes, keys that arrive later bring it.
            let mut bg_last = String::new();
            let mut bg_checked: Option<std::time::Instant> = None;
            // only the newest resolution may publish (an older, slower one could otherwise bring back a deleted image)
            let bg_seq = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
            let mut first = true;
            let mut fail_streak: u32 = 0;
            loop {
                // Load the timeline at the current window depth (grows via load_older) and push it
                // to QML; `is_current` drops a result that belongs to an earlier open.
                let want = limit2.load(Ordering::Relaxed).max(DEFAULT_WINDOW);
                let t_load = std::time::Instant::now();
                // type-erased: a deep future (rustc's query depth limit on this task's Send check)
                let fut: std::pin::Pin<Box<dyn std::future::Future<Output = Result<Loaded, String>> + Send + '_>> = Box::pin(load_room_data(&room, want, &view));
                let data = fut.await;
                let failed = data.is_err();
                if first {
                    eprintln!(
                        "[room] {room_id} gen={gen}: live timeline after {} ms (load {} ms){}",
                        opened.elapsed().as_millis(),
                        t_load.elapsed().as_millis(),
                        if failed { " — FAILED" } else { "" }
                    );
                }
                let started = room_id.clone();
                let _ = qt.queue(move |mut o| {
                    if !is_current(&o, gen, &started) {
                        return;
                    }
                    o.as_mut().set_busy(false);
                    o.as_mut().set_loading_older(false);
                    match data {
                        Ok(l) => {
                            l.apply(o.as_mut());
                            if o.status().to_string().starts_with("Couldn't reach the server") {
                                o.as_mut().set_status(QString::default());
                            }
                        }
                        Err(e) => {
                            eprintln!("[room] load failed: {e}");
                            // 10.1 (TL-06): say so — the timeline used to draw "No messages" for a dead link.
                            o.as_mut().set_status(QString::from("Couldn't reach the server — showing what's cached. Trying again…"));
                        }
                    }
                });
                // Right-panel data (members + summary) + our own power level / moderation
                // affordances — after the timeline is on screen. members() syncs from the
                // homeserver only on its first call.
                if state_changed {
                    state_changed = false;
                    // the room's shared background (docs/AMBIENCE.md): re-read with every batch that carried state —
                    // content and sender from ONE state event; the image is resolved in its own task (below)
                    {
                        let ty = tinhat_widgets::ambience::BACKGROUND_STATE;
                        let bg = match crate::appstate::get_state_with_sender(&room, ty, "").await {
                            Some((content, sender)) => serde_json::json!({ "content": content, "sender": sender }).to_string(),
                            None => String::new(),
                        };
                        // bgsheet-1220: power levels that can't be read are "not known", not "may not"
                        let (can, known) = match room.power_levels().await {
                            Ok(pl) => (pl.user_can_send_state(room.own_user_id(), matrix_sdk::ruma::events::StateEventType::from(ty)), true),
                            Err(_) => (false, false),
                        };
                        let started = room_id.clone();
                        let bg2 = bg.clone();
                        let _ = qt.queue(move |mut o| {
                            if is_current(&o, gen, &started) {
                                if o.background_json().to_string() != bg2 {
                                    // a new background: the old image never shows under it (its task brings the new one)
                                    o.as_mut().set_background_image(QString::default());
                                    o.as_mut().set_background_image_cells(QString::default());
                                    o.as_mut().set_background_json(QString::from(&bg2));
                                }
                                o.as_mut().set_can_set_background(can);
                                o.as_mut().set_can_set_background_known(known);
                            }
                        });
                        bg_last = bg;
                        bg_checked = None;
                    }
                    let m = load_members(&room).await;
                    let perms = load_permissions(&room).await;
                    let started = room_id.clone();
                    let _ = qt.queue(move |mut o| {
                        if !is_current(&o, gen, &started) {
                            return;
                        }
                        o.as_mut().set_members_json(QString::from(&m.0));
                        o.as_mut().set_room_summary_json(QString::from(&m.1));
                        perms.apply(o.as_mut());
                    });
                }
                if bg_checked.is_none_or(|t| t.elapsed() > std::time::Duration::from_secs(20)) {
                    bg_checked = Some(std::time::Instant::now());
                    let (room2, bg, qt2, started) = (room.clone(), bg_last.clone(), qt.clone(), room_id.clone());
                    let seq = bg_seq.clone();
                    let mine = seq.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                    crate::bridge::runtime().spawn(async move {
                        let (url, cells) = background_image_file(&room2, &bg).await;
                        let _ = qt2.queue(move |mut o| {
                            // only the newest resolution, for the background it was resolved for
                            if seq.load(std::sync::atomic::Ordering::SeqCst) == mine && is_current(&o, gen, &started) && o.background_json().to_string() == bg {
                                if o.background_image_cells().to_string() != cells {
                                    o.as_mut().set_background_image_cells(QString::from(&cells));
                                }
                                if o.background_image().to_string() != url {
                                    o.as_mut().set_background_image(QString::from(&url));
                                }
                            }
                        });
                    });
                }
                let mut reload_now = false;
                if first {
                    first = false;
                    // Pull this room's megolm keys from key backup (best-effort; needs a verified
                    // device) so encrypted history decrypts: once per room per session, and again
                    // whenever the window still holds messages we can't read. Rows that become
                    // readable repaint straight away (redecrypt_window, keyed off the keys epoch).
                    if backup_download_wanted(&room_id, window_has_undecryptable(&room_id).await) {
                        let t_keys = std::time::Instant::now();
                        match client.encryption().backups().download_room_keys_for_room(&rid).await {
                            Ok(()) => {
                                eprintln!(
                                    "[room] backup key download OK for {rid} ({} ms, after the first paint)",
                                    t_keys.elapsed().as_millis()
                                );
                                note_backup_downloaded(&room_id);
                                bump_keys_epoch(&room_id);
                                reload_now = window_has_undecryptable(&room_id).await;
                            }
                            Err(e) => eprintln!("[room] backup key download FAILED for {rid}: {e}"),
                        }
                    }
                    // If we're an admin here, make sure every Tin Hat member-state event type is
                    // sendable by power-level-0 members (one power_levels write) — once per room per
                    // session and only in my own server's rooms (the must-fix round,
                    // MATRIX-FRONTEND-DESIGN.md B5 rule 1: this used to rewrite rooms on other servers
                    // too). The web's ConversationView does the same.
                    crate::appstate::provision_on_open(&room).await;
                }
                fail_streak = if failed { fail_streak.saturating_add(1) } else { 0 };
                // Wait for the next reload-worthy update (see classify_update):
                // typing-only updates never reload (the typing task renders those);
                // receipt-only updates reload under RECEIPT_COALESCE so receipt churn
                // can't storm /messages; anything carrying timeline events (incl.
                // redactions), a `limited` gap, or state changes reloads after a
                // short debounce so bursts collapse into one refetch.
                // 10.5.2: a failed load now retries on its own (2 s, 4 s, … 30 s) — "Trying
                // again…" used to wait for the next thing to happen in the room (forever in a
                // quiet one); keys that just made rows readable reload at once.
                let reloaded_at = tokio::time::Instant::now();
                let mut deadline: Option<tokio::time::Instant> = if reload_now {
                    Some(reloaded_at)
                } else if failed {
                    Some(reloaded_at + retry_backoff(fail_streak))
                } else {
                    None
                };
                let mut urgent = reload_now; // a Reload-class update is already pending
                let closed = loop {
                    // 10.5.2: keys that arrive on their own (a late share, a backup import) wake the
                    // loop too — they need not come with any room event.
                    let next = async {
                        use futures_util::future::{select, Either};
                        let upd = Box::pin(rx.recv());
                        let keys = Box::pin(next_keys_for(&mut keys_rx, &room_id));
                        match select(upd, keys).await {
                            Either::Left((r, _)) => Wake::Update(r),
                            Either::Right((echo, _)) => Wake::Keys(echo),
                        }
                    };
                    let wake = match deadline {
                        Some(d) => match tokio::time::timeout_at(d, next).await {
                            Ok(w) => w,
                            Err(_) => break false, // window elapsed -> reload
                        },
                        None => next.await,
                    };
                    let class = match wake {
                        // L1 D08.2: this room's local echoes changed (resent after a reconnect): repaint them
                        Wake::Keys(true) => UpdateClass::Reload { state: false },
                        Wake::Keys(false) if window_has_undecryptable(&room_id).await => UpdateClass::Reload { state: false },
                        Wake::Keys(false) => UpdateClass::Skip,
                        Wake::Update(Ok(update)) => {
                            note_newest_nonrow(room.room_id().as_str(), &update); // 10.3
                            classify_update(&update)
                        }
                        // Missed updates could have carried anything: full reload.
                        Wake::Update(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {
                            UpdateClass::Reload { state: true }
                        }
                        Wake::Update(Err(_)) => break true, // channel closed (room gone / logout)
                    };
                    match class {
                        UpdateClass::Skip => {}
                        UpdateClass::Receipt if deadline.is_none() => {
                            // No sooner than RECEIPT_COALESCE after the last reload
                            // (and at least one debounce from now).
                            deadline = Some(
                                (reloaded_at + RECEIPT_COALESCE)
                                    .max(tokio::time::Instant::now() + RELOAD_DEBOUNCE),
                            );
                        }
                        UpdateClass::Receipt => {} // an earlier deadline already covers it
                        UpdateClass::Reload { state } => {
                            if state {
                                state_changed = true;
                            }
                            if !urgent {
                                urgent = true;
                                let at = tokio::time::Instant::now() + RELOAD_DEBOUNCE;
                                // A pending receipt wait may be longer; take the sooner.
                                deadline = Some(deadline.map_or(at, |d| d.min(at)));
                            }
                            // else: the debounce window is already running
                        }
                    }
                };
                if closed {
                    break; // channel closed (room gone / logout)
                }
            }
        });
        self.as_mut().rust_mut().sub_task = Some(task);
    }

    /// Tear down this controller's background work. Aborts the live-subscription and
    /// typing tasks (same as `open()` does on a room switch) so a closed floating bubble
    /// does not leave detached tokio loops running for the rest of the session. Safe to
    /// call more than once (the handles are `take()`n). Sends no receipts.
    pub fn close(mut self: Pin<&mut Self>) {
        if let Some(t) = self.as_mut().rust_mut().sub_task.take() {
            t.abort();
        }
        if let Some(t) = self.as_mut().rust_mut().typing_task.take() {
            t.abort();
        }
        // A closed view can't be looked at (TH-NOTIF-01: the old single slot was never cleared).
        let token = self.as_mut().attend_token();
        crate::notify::set_attending(token, None);
    }

    /// 10.1: this view's token in the notifier's attended-rooms table, assigned on first use.
    fn attend_token(mut self: Pin<&mut Self>) -> u64 {
        if self.rust().attend_token == 0 {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
            let t = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.as_mut().rust_mut().attend_token = t;
        }
        self.rust().attend_token
    }

    pub fn set_attending(mut self: Pin<&mut Self>, on: bool) {
        let room_id = self.room_id().to_string();
        let token = self.as_mut().attend_token();
        crate::notify::set_attending(token, if on && !room_id.is_empty() { Some(room_id) } else { None });
    }

    pub fn refresh(mut self: Pin<&mut Self>) {
        let room_id = self.room_id().to_string();
        if room_id.is_empty() {
            return;
        }
        // No `busy` early-return: switching rooms must supersede an in-flight load.
        // Stale results are dropped in the queue closure via the room_id stamp.
        self.as_mut().set_busy(true);

        let want = self.as_mut().rust().loaded_limit.load(Ordering::Relaxed).max(DEFAULT_WINDOW);
        let gen = self.rust().open_gen;
        let qt = self.qt_thread();
        let started_for = room_id.clone();
        let view = self.thread_root().to_string();
        crate::bridge::runtime().spawn(async move {
            let result = load_room(&room_id, want, &view).await;
            let mdata = load_members_by_id(&room_id).await;
            let perms = load_permissions_by_id(&room_id).await;
            let _ = qt.queue(move |mut o| {
                // Ignore a result for a room the user has since navigated away from
                // (fast room switching) — otherwise a slow load clobbers the new room.
                // 10.5.2: … or for an earlier open of this same room.
                if !is_current(&o, gen, &started_for) {
                    return;
                }
                o.as_mut().set_busy(false);
                o.as_mut().set_loading_older(false);
                o.as_mut().set_members_json(QString::from(&mdata.0));
                o.as_mut().set_room_summary_json(QString::from(&mdata.1));
                perms.apply(o.as_mut());
                match result {
                    Ok(l) => l.apply(o.as_mut()),
                    Err(e) => {
                        eprintln!("[room] load failed: {e}");
                        o.as_mut().set_status(QString::from(&format!("Couldn't load room: {e}")));
                    }
                }
            });
        });
    }

    /// Send our typing notice to the open room (called debounced from the composer).
    pub fn set_typing(self: Pin<&mut Self>, typing: bool) {
        let room_id = self.room_id().to_string();
        if room_id.is_empty() {
            return;
        }
        crate::bridge::runtime().spawn(async move {
            if let Ok(room) = room_by_id(&room_id).await {
                if let Err(e) = room.typing_notice(typing).await {
                    eprintln!("[room] typing_notice failed: {e}");
                }
            }
        });
    }

    /// Grow the loaded-history window by one page and re-render (older messages get
    /// prepended). The live-subscription task picks up the new depth on its next pass;
    /// we also kick a one-shot load for immediate feedback.
    pub fn load_older(mut self: Pin<&mut Self>) {
        let room_id = self.room_id().to_string();
        if room_id.is_empty() || *self.loading_older() {
            return;
        }
        if !*self.has_more() {
            return; // already at the start of history
        }
        self.as_mut().set_loading_older(true);
        let want = self
            .as_mut()
            .rust()
            .loaded_limit
            .fetch_add(WINDOW_STEP, Ordering::Relaxed)
            + WINDOW_STEP;

        let gen = self.rust().open_gen;
        let qt = self.qt_thread();
        let started_for = room_id.clone();
        let view = self.thread_root().to_string();
        crate::bridge::runtime().spawn(async move {
            let result = load_room(&room_id, want, &view).await;
            let _ = qt.queue(move |mut o| {
                if !is_current(&o, gen, &started_for) {
                    return;
                }
                o.as_mut().set_loading_older(false);
                if std::mem::take(&mut o.as_mut().rust_mut().trim_pending) {
                    let limit = &o.rust().loaded_limit;
                    limit.store(window_after_jump_to_latest(limit.load(Ordering::Relaxed)), Ordering::Relaxed);
                }
                match result {
                    Ok(l) => l.apply(o.as_mut()),
                    Err(e) => {
                        // 10.1 (TH-STATE-10): the spinner used to just vanish.
                        eprintln!("[room] load_older failed: {e}");
                        o.as_mut().set_status(QString::from("Couldn't load older messages — scroll up to try again"));
                    }
                }
            });
        });
    }

    /// A35.4: see the bridge declaration. A page still loading keeps its depth (it was asked for).
    pub fn trim_to_latest(mut self: Pin<&mut Self>) {
        if *self.loading_older() {
            // the page still loading lands first (it was asked for), then the window shrinks (load_older applies this)
            self.as_mut().rust_mut().trim_pending = true;
            return;
        }
        let limit = &self.rust().loaded_limit;
        limit.store(window_after_jump_to_latest(limit.load(Ordering::Relaxed)), Ordering::Relaxed);
    }

    pub fn mark_read(self: Pin<&mut Self>) {
        let room_id = self.room_id().to_string();
        let json = self.messages_json().to_string();
        // threads (MSC3771): the main timeline's receipt is "main" (reading it never clears a thread); a thread's
        // panel sends its thread's (thread_id = the root) on its newest event
        let thread = self.thread_root().to_string();
        if room_id.is_empty() {
            return;
        }
        crate::bridge::runtime().spawn(async move {
            // Newest loaded message = the last element that is a server event (9.3: local
            // echo rows sit at the tail with a "~txn" id the server has never seen).
            let latest = serde_json::from_str::<serde_json::Value>(&json)
                .ok()
                .and_then(|v| {
                    v.as_array().and_then(|a| {
                        a.iter().rev().find(|m| m.get("echo").and_then(|e| e.as_bool()) != Some(true)).cloned()
                    })
                })
                .and_then(|m| {
                    let id = m.get("id").and_then(|i| i.as_str())?.to_owned();
                    Some((id, m.get("ts").and_then(|t| t.as_u64()).unwrap_or(0)))
                });
            // 10.3: housekeeping newer than the newest bubble is read too — the receipt goes there, or the server keeps
            // counting it (see newest_nonrow). 12.1.17 round 2 (review #2): a thread's panel has its own (its photos the
            // chat hides), so a hidden tail — even a thread whose only reply is hidden — is read too.
            let nonrow = newest_nonrow().lock().ok().and_then(|g| g.get(&nonrow_key(&room_id, &thread)).cloned());
            let Some(id) = receipt_target(latest, nonrow, &thread) else {
                // threads: a thread's panel that shows only its root and has nothing hidden has nothing to read yet (the
                // root sits in the main timeline; a thread receipt on it would be refused)
                if !thread.is_empty() {
                    return;
                }
                // 10.6 (REACH A5.6, audit 15a): a room with no chat bubble yet — an accepted DM
                // nobody has written in — still gets its receipt, on its newest event of any kind
                // (what "Mark as read" does), or the invite's own count stays and the DM would sit
                // on the rail's strip for good. Only while the server counts something, and at most
                // once a minute per room (this runs on every timeline refresh).
                empty_room_receipt(&room_id).await;
                return;
            };
            #[cfg(tinhat_env_selftests)] // never in a release (build.rs): a gate hook (12.1.18 assembly fence)
            if std::env::var_os("TINHAT_GUI_MEMHIDE").is_some() {
                eprintln!("[memhide] receipt room={room_id} thread={} target={id}", if thread.is_empty() { "main" } else { &thread });
            }
            let Ok(room) = room_by_id(&room_id).await else { return };
            if let Ok(eid) = matrix_sdk::ruma::EventId::parse(&id) {
                // public m.read only in a room of my own server, else m.read.private (receipts.rs)
                let _ = crate::receipts::send_read_receipt(&room, &eid, if thread.is_empty() { "main" } else { &thread }).await;
            }
        });
    }

    pub fn load_more_threads(mut self: Pin<&mut Self>) {
        let n = self.rust().threads_pages.max(1) + 1;
        self.as_mut().rust_mut().threads_pages = n;
        self.load_threads();
    }

    pub fn thread_summary_text(&self, count: i32, name: &QString, ts: f64) -> QString {
        QString::from(&tinhat_widgets::threads::summary_text(count as i64, &name.to_string(), ts as i64, now_ms() as i64))
    }
    pub fn thread_of_event(&self, event_id: &QString) -> QString {
        let id = event_id.to_string();
        let rid = self.room_id().to_string();
        if id.is_empty() || rid.is_empty() {
            return QString::default();
        }
        let win = timeline_window_for(&rid);
        let Ok(w) = win.try_lock() else { return QString::default() };
        let t = w
            .events
            .iter()
            .find(|v| crate::threads_rows::ev_id(v) == id)
            .map(|v| tinhat_widgets::threads::thread_of(v.get("content").unwrap_or(&serde_json::Value::Null)))
            .unwrap_or_default();
        QString::from(&t)
    }

    pub fn thread_summary_label(&self, count: i32, name: &QString, ts: f64) -> QString {
        QString::from(&tinhat_widgets::threads::summary_label(count as i64, &name.to_string(), ts as i64, now_ms() as i64))
    }

    /// threads (docs/THREADS.md): the room's threads for the Threads list → threadsJson
    /// `{items: [{root, rootSender, rootName, rootAvatar, snippet, count, lastName, lastTs, unread, mine}], all: [root…],
    /// mine: [root…]}` (the two orders by tinhat_widgets::threads::thread_list, the web's threadList).
    pub fn load_threads(mut self: Pin<&mut Self>) {
        let room_id = self.room_id().to_string();
        if room_id.is_empty() {
            return;
        }
        let gen = self.rust().open_gen;
        // review #12 / round 2 #13: only this controller's newest request's answer lands (a slow older one never
        // overwrites it; another controller's requests don't count)
        let seq = self.rust().threads_seq + 1;
        self.as_mut().rust_mut().threads_seq = seq;
        let pages = self.rust().threads_pages.max(1);
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let Ok(room) = room_by_id(&room_id).await else { return };
            let json = Box::pin(load_threads_json(&room, pages)).await;
            let started = room_id.clone();
            let _ = qt.queue(move |mut o| {
                if is_current(&o, gen, &started) && o.rust().threads_seq == seq {
                    o.as_mut().set_threads_json(QString::from(&json));
                }
            });
        });
    }

    /// Resolve a media event to a cached local file and announce it via `mediaOpened`.
    /// The raw (post-decryption) event comes from the in-memory TimelineWindow, so the
    /// encrypted-file key material is present for E2EE rooms. The property is blanked
    /// first so re-opening the SAME file still fires the QML change signal.
    pub fn open_media(mut self: Pin<&mut Self>, event_id: &QString) {
        let event_id = event_id.to_string();
        let room_id = self.room_id().to_string();
        if event_id.is_empty() || room_id.is_empty() {
            return;
        }
        self.as_mut().set_media_opened(QString::default());
        self.as_mut().set_media_text(QString::default());
        self.as_mut().set_media_busy(QString::from(&event_id));   // 10.1 (TH-MSG-13): the chip spins while this runs
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let win_arc = timeline_window_for(&room_id);
            let (content, mime) = {
                let win = win_arc.lock().await;
                let Some(v) = win
                    .events
                    .iter()
                    .find(|v| v.get("event_id").and_then(|e| e.as_str()) == Some(&event_id))
                else {
                    eprintln!("[room] open_media: event not in window: {event_id}");
                    let _ = qt.queue(|mut o| {
                        o.as_mut().set_media_busy(QString::default());
                        o.as_mut().set_status(QString::from("That attachment isn't loaded any more — scroll to it and try again"));
                    });
                    return;
                };
                let mime = v
                    .get("content")
                    .and_then(|c| c.get("info"))
                    .and_then(|i| i.get("mimetype"))
                    .and_then(|m| m.as_str())
                    .map(String::from);
                (v.get("content").cloned().unwrap_or_default(), mime)
            };
            // launch-gate's web review #8: a text-like file (plain text, JSON, CSV, Markdown, logs, source) is shown in the
            // app as plain text — never handed to an outside opener
            let name = attachment_name(&content);
            let msgtype = content.get("msgtype").and_then(|m| m.as_str()).unwrap_or("").to_string();
            if open_as(&msgtype, true, mime.as_deref().unwrap_or(""), &name) == OpenAs::Text {
                let shown = match fetch_media(content, &event_id, mime.as_deref(), MediaUse::Text).await {
                    Ok(url) => tokio::fs::read(crate::platform::path_from_file_url(&url)).await.map_err(|_| MediaRefused::Failed),
                    Err(e) => Err(e),
                };
                let _ = qt.queue(move |mut o| {
                    o.as_mut().set_media_busy(QString::default());
                    match shown {
                        Ok(bytes) => {
                            let text = String::from_utf8_lossy(&bytes);
                            let json = serde_json::json!({ "id": event_id, "name": name, "text": text.trim_start_matches('\u{feff}') });
                            o.as_mut().set_media_text(QString::from(&json.to_string()));
                        }
                        Err(why) => {
                            eprintln!("[room] open_media (text): {why:?} for {event_id}");
                            o.as_mut().set_status(QString::default());
                            o.as_mut().set_status(QString::from(why.text_toast()));
                        }
                    }
                });
                return;
            }
            // security review 2026-10-26: the OS opener gets only a viewer type whose bytes are that type (viewer.rs)
            match fetch_media(content, &event_id, mime.as_deref(), MediaUse::Open).await {
                Ok(url) => {
                    let _ = qt.queue(move |mut o| {
                        o.as_mut().set_media_busy(QString::default());
                        o.as_mut().set_media_opened(QString::from(&url));
                    });
                }
                Err(why) => {
                    eprintln!("[room] open_media: {why:?} for {event_id} ({})", mime.as_deref().unwrap_or("no type"));
                    let _ = qt.queue(move |mut o| {
                        o.as_mut().set_media_busy(QString::default());
                        // blank first so the same refusal twice still toasts
                        o.as_mut().set_status(QString::default());
                        o.as_mut().set_status(QString::from(why.toast()));
                    });
                }
            }
        });
    }

    /// Resolve an attachment to a local file for INLINE playback.
    pub fn resolve_media(mut self: Pin<&mut Self>, event_id: &QString) {
        let event_id = event_id.to_string();
        let room_id = self.room_id().to_string();
        if event_id.is_empty() || room_id.is_empty() {
            return;
        }
        // Announce the pending id immediately: the bubble shows a spinner off this, and
        // a large video can take a while to arrive.
        self.as_mut().set_media_resolved(QString::from(
            &serde_json::json!({ "id": event_id, "url": "", "loading": true }).to_string(),
        ));
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let win_arc = timeline_window_for(&room_id);
            let (content, mime) = {
                let win = win_arc.lock().await;
                let Some(v) = win
                    .events
                    .iter()
                    .find(|v| v.get("event_id").and_then(|e| e.as_str()) == Some(&event_id))
                else {
                    return;
                };
                let mime = v
                    .get("content")
                    .and_then(|c| c.get("info"))
                    .and_then(|i| i.get("mimetype"))
                    .and_then(|m| m.as_str())
                    .map(String::from);
                (v.get("content").cloned().unwrap_or_default(), mime)
            };
            let url = fetch_media(content, &event_id, mime.as_deref(), MediaUse::Play)
                .await
                .unwrap_or_default();
            if url.is_empty() {
                eprintln!("[room] resolve_media: failed for {event_id}");
            }
            let json = serde_json::json!({ "id": event_id, "url": url, "loading": false })
                .to_string();
            let _ = qt.queue(move |mut o| o.as_mut().set_media_resolved(QString::from(&json)));
        });
    }

    /// Save an attachment to a location the user picked.
    pub fn save_media_as(mut self: Pin<&mut Self>, event_id: &QString, dest: &QString) {
        let event_id = event_id.to_string();
        let room_id = self.room_id().to_string();
        // QML hands back a file:// url from the save dialog; Win32 and std::fs both want
        // a path (and on Windows the url carries a leading slash before the drive letter).
        let dest = crate::platform::path_from_file_url(&dest.to_string());
        if event_id.is_empty() || room_id.is_empty() || dest.is_empty() {
            return;
        }
        self.as_mut().set_status(QString::from("Saving…"));
        self.as_mut().set_media_busy(QString::from(&event_id));   // 10.1 (TH-MSG-13)
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let win_arc = timeline_window_for(&room_id);
            let (content, mime) = {
                let win = win_arc.lock().await;
                let Some(v) = win
                    .events
                    .iter()
                    .find(|v| v.get("event_id").and_then(|e| e.as_str()) == Some(&event_id))
                else {
                    let _ = qt.queue(|mut o| {
                        o.as_mut().set_media_busy(QString::default());
                        o.as_mut().set_status(QString::from("That attachment isn't loaded any more — scroll to it and try again"));
                    });
                    return;
                };
                let mime = v
                    .get("content")
                    .and_then(|c| c.get("info"))
                    .and_then(|i| i.get("mimetype"))
                    .and_then(|m| m.as_str())
                    .map(String::from);
                (v.get("content").cloned().unwrap_or_default(), mime)
            };
            let msg = match fetch_media(content, &event_id, mime.as_deref(), MediaUse::Save).await.ok() {
                Some(url) => {
                    let src = crate::platform::path_from_file_url(&url);
                    match tokio::fs::copy(&src, &dest).await {
                        Ok(_) => format!("Saved to {dest}"),   // 10.1 (TH-MSG-13): say where
                        Err(e) => format!("Couldn't save: {e}"),
                    }
                }
                None => "Couldn't download that file".to_string(),
            };
            let _ = qt.queue(move |mut o| {
                o.as_mut().set_media_busy(QString::default());
                // Blank first so saving the same file twice still toasts (the mirror only
                // reacts to a CHANGE).
                o.as_mut().set_status(QString::default());
                o.as_mut().set_status(QString::from(&msg));
            });
        });
    }

    pub fn local_file_size(&self, url: &QString) -> i64 {
        let path = crate::platform::path_from_file_url(&url.to_string());
        std::fs::metadata(&path).map(|m| m.len().min(i64::MAX as u64) as i64).unwrap_or(0)
    }

    pub fn send_attachment(mut self: Pin<&mut Self>, local_path: &QString) {
        let room_id = self.room_id().to_string();
        let path_or_url = local_path.to_string();
        if room_id.is_empty() || path_or_url.is_empty() {
            return;
        }
        self.as_mut().set_status(QString::default());
        // 9.3: the file shows up in the timeline NOW (an image renders from the local file
        // while it uploads), dimmed until the server confirms it — the row is a local echo.
        let path = crate::platform::path_from_file_url(&path_or_url);
        let pb = std::path::PathBuf::from(&path);
        let filename = pb.file_name().and_then(|f| f.to_str()).unwrap_or("file").to_string();
        let ext = pb.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        let mime = mime_for_ext(&ext).to_string();
        let msgtype = if mime.starts_with("image/") {
            "m.image"
        } else if mime.starts_with("video/") {
            "m.video"
        } else if mime.starts_with("audio/") {
            "m.audio"
        } else {
            "m.file"
        };
        let size = std::fs::metadata(&pb).map(|m| m.len()).unwrap_or(0);
        let mut echo = Echo::new(EchoKind::Attachment(path.clone()), &filename);
        echo.msgtype = msgtype.to_string();
        echo.mime = mime;
        echo.size = size;
        if msgtype == "m.image" {
            // HEIC/HEIF/AVIF: no shipped Qt draws them, so the echo shows the same file card the sent message will
            if !matches!(ext.as_str(), "heic" | "heif" | "hif" | "avif") {
                echo.media = crate::platform::file_url(&pb);
            }
            if let Some((w, h)) = crate::clipboard::image_size(&pb) {
                echo.w = w;
                echo.h = h;
            }
        }
        self.as_mut().start_echo(echo);
    }

    pub fn send_voice_note(mut self: Pin<&mut Self>, request_json: &QString) {
        let room_id = self.room_id().to_string();
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&request_json.to_string()) else { return };
        let txn = v.get("txn").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let path = v.get("path").and_then(|x| x.as_str()).unwrap_or("").to_string();
        if txn.is_empty() {
            return;
        }
        if room_id.is_empty() || path.is_empty() || v.get("room").and_then(|x| x.as_str()) != Some(room_id.as_str()) {
            // the view moved on between the request and here: the draft goes back to review, nothing is sent
            crate::voicenote_ctl::report_send(&txn, Err("Couldn't send — try again".into()));
            return;
        }
        let send = VoiceSend {
            path: path.clone(),
            duration_ms: v.get("durationMs").and_then(|x| x.as_u64()).unwrap_or(0),
            waveform: v.get("waveform").and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|w| w.as_u64()).map(|w| w.min(1024) as u32).collect()).unwrap_or_default(),
            size: v.get("size").and_then(|x| x.as_u64()).unwrap_or(0),
        };
        self.as_mut().set_status(QString::default());
        // a retry: the row this transaction already has goes again (one local echo per note)
        if let Some(mut e) = echo_store_take(&room_id, &txn) {
            e.failed = None;
            e.event_id = None;
            e.kind = EchoKind::Voice(send);
            self.as_mut().start_echo(e);
            return;
        }
        let mut echo = Echo::new(EchoKind::Voice(send.clone()), tinhat_widgets::voicenote::VOICE_BODY);
        echo.txn = txn;
        echo.msgtype = "m.audio".into();
        echo.mime = "audio/ogg".into();
        echo.size = send.size;
        echo.duration_ms = send.duration_ms;
        echo.media = crate::platform::file_url(std::path::Path::new(&path));
        self.as_mut().start_echo(echo);
    }

    pub fn send(mut self: Pin<&mut Self>, text: &QString) {
        let text = text.to_string();
        if text.trim().is_empty() || self.room_id().is_empty() {
            return;
        }
        self.as_mut().set_status(QString::default());
        self.as_mut().start_echo(Echo::new(EchoKind::Text, &text));
    }

    /// 9.3.2: what Ctrl+V should attach (see `clipboard::attachments`). Pasted images land in
    /// the media cache next to decrypted inline media, so the startup LRU sweep reclaims them.
    pub fn clipboard_attachments(&self) -> QString {
        let urls = crate::clipboard::attachments(&media_cache_dir());
        let arr = serde_json::Value::Array(urls.into_iter().map(serde_json::Value::String).collect());
        QString::from(arr.to_string().as_str())
    }

    /// 9.3: `/me` — an m.emote through the local-echo path.
    pub fn send_emote(mut self: Pin<&mut Self>, text: &QString) {
        let text = text.to_string();
        if text.trim().is_empty() || self.room_id().is_empty() {
            return;
        }
        self.as_mut().set_status(QString::default());
        self.as_mut().start_echo(Echo::new(EchoKind::Emote, &text));
    }

    /// widgets W3: a dice roll into the open room (the local-echo path: pending / sent / failed + Retry like any message).
    pub fn send_dice(mut self: Pin<&mut Self>, content_json: &QString) {
        let Ok(content) = serde_json::from_str::<serde_json::Value>(&content_json.to_string()) else { return };
        if tinhat_widgets::dice::read_dice(&content).is_none() || self.room_id().is_empty() {
            return;
        }
        let body = content.get("body").and_then(|b| b.as_str()).unwrap_or("").to_string();
        self.as_mut().set_status(QString::default());
        self.as_mut().start_echo(Echo::new(EchoKind::Dice(content), &body));
    }

    /// widgets W3: a new poll into the open room (the local-echo path: the card shows at once, pending / failed + Retry).
    pub fn send_poll(mut self: Pin<&mut Self>, question: &QString, answers_json: &QString, disclosed: bool) -> QString {
        if self.room_id().is_empty() {
            return QString::from("Open a room first");
        }
        let answers: Vec<String> = serde_json::from_str(&answers_json.to_string()).unwrap_or_default();
        let ids: Vec<String> = answers.iter().map(|_| poll_answer_id()).collect();
        match tinhat_widgets::polls::start_content(&question.to_string(), &answers, disclosed, &ids) {
            Err((_, message)) => QString::from(message),
            Ok(content) => {
                let q = tinhat_widgets::polls::read_start(tinhat_widgets::polls::START_TYPE, &content).map(|p| p.question).unwrap_or_default();
                self.as_mut().set_status(QString::default());
                self.as_mut().start_echo(Echo::new(EchoKind::Poll(content), &format!("Poll: {q}")));
                QString::default()
            }
        }
    }

    /// widgets W3: a vote — a local response joins the window's relations at once (the card and the widget follow), then
    /// the real send; a failed send takes the local one out again.
    pub fn vote_poll(mut self: Pin<&mut Self>, start: &QString, answers_json: &QString) {
        let start = start.to_string();
        let room_id = self.room_id().to_string();
        let answers: Vec<String> = serde_json::from_str(&answers_json.to_string()).unwrap_or_default();
        if room_id.is_empty() || !start.starts_with('$') || start.starts_with("$w3-sample-") || answers.is_empty() {
            return;
        }
        let content = tinhat_widgets::polls::response_content(&start, &answers);
        self.as_mut().poll_local_send(room_id, tinhat_widgets::polls::RESPONSE_TYPE, content);
    }

    /// widgets W3: end a poll — a local end joins the window's relations at once, then the real send.
    pub fn end_poll(mut self: Pin<&mut Self>, start: &QString) {
        let start = start.to_string();
        let room_id = self.room_id().to_string();
        if room_id.is_empty() || !start.starts_with('$') || start.starts_with("$w3-sample-") {
            return;
        }
        let me = crate::session::client().and_then(|c| c.user_id().map(|u| u.to_string())).unwrap_or_default();
        let msgs: Vec<serde_json::Value> = serde_json::from_str(&self.messages_json().to_string()).unwrap_or_default();
        let Some(row) = msgs.iter().find(|m| m.get("id").and_then(|i| i.as_str()) == Some(start.as_str())) else { return };
        let ty = row.get("pollType").and_then(|t| t.as_str()).unwrap_or("");
        let Some(poll) = tinhat_widgets::polls::read_start(ty, row.get("pollContent").unwrap_or(&serde_json::Value::Null)) else { return };
        let creator = row.get("sender").and_then(|s| s.as_str()).unwrap_or("");
        let t = tinhat_widgets::polls::tally(&poll, &start, creator, &self.rust().poll_rels, &self.rust().poll_enders, &me);
        let content = tinhat_widgets::polls::end_content(&start, &tinhat_widgets::polls::end_text(&poll, &t));
        self.as_mut().poll_local_send(room_id, tinhat_widgets::polls::END_TYPE, content);
    }

    fn poll_local_send(mut self: Pin<&mut Self>, room_id: String, ty: &'static str, content: serde_json::Value) {
        let me = crate::session::client().and_then(|c| c.user_id().map(|u| u.to_string())).unwrap_or_default();
        let local_id = format!("~poll-{}-{}", now_ms(), poll_answer_id());
        self.as_mut().rust_mut().poll_rels.push(serde_json::json!({ "id": local_id, "type": ty, "sender": me, "ts": now_ms(), "content": content }));
        self.as_mut().refresh_poll_cards();
        let qt = self.qt_thread();
        let gen = self.rust().open_gen;
        crate::bridge::runtime().spawn(async move {
            let res = match room_by_id(&room_id).await {
                Ok(room) => room.send_raw(ty, content).await.map(|_| ()).map_err(|e| e.to_string()),
                Err(e) => Err(e),
            };
            if let Err(e) = res {
                eprintln!("[polls] {ty} failed: {e}");
                let _ = qt.queue(move |mut o| {
                    if o.rust().open_gen != gen {
                        return;
                    }
                    o.as_mut().rust_mut().poll_rels.retain(|r| r.get("id").and_then(|i| i.as_str()) != Some(local_id.as_str()));
                    o.as_mut().refresh_poll_cards();
                    o.as_mut().set_status(QString::from("Couldn't send that to the poll — try again"));
                });
            }
        });
    }

    /// widgets W3: rebuild every poll card of the shown rows from the stored relations (after a local vote / end).
    fn refresh_poll_cards(mut self: Pin<&mut Self>) {
        let me = crate::session::client().and_then(|c| c.user_id().map(|u| u.to_string())).unwrap_or_default();
        let mut msgs: Vec<serde_json::Value> = serde_json::from_str(&self.messages_json().to_string()).unwrap_or_default();
        let (rels, enders, can) = (self.rust().poll_rels.clone(), self.rust().poll_enders.clone(), self.rust().poll_can_redact);
        attach_poll_cards(&mut msgs, &rels, &enders, &me, can);
        let polls = poll_rows(&msgs).to_string();
        self.as_mut().set_messages_json(QString::from(&serde_json::Value::Array(msgs).to_string()));
        self.as_mut().set_polls_json(QString::from(&polls));
    }

    /// 9.3: resend a failed local echo (same row, same txn — the QML key never changes).
    pub fn retry_send(mut self: Pin<&mut Self>, txn: &QString) {
        let room_id = self.room_id().to_string();
        let txn = txn.to_string();
        let Some(mut e) = echo_store_take(&room_id, &txn) else { return };
        e.failed = None;
        e.auto_tries = 0; // L1 D08.2: a Retry starts the automatic count over
        e.event_id = None;
        e.ts = now_ms();
        self.as_mut().start_echo(e);
    }

    /// 9.3: forget a failed local echo (row + store).
    pub fn discard_echo(mut self: Pin<&mut Self>, txn: &QString) {
        let room_id = self.room_id().to_string();
        let txn = txn.to_string();
        let _ = echo_store_take(&room_id, &txn);
        let cur = self.messages_json().to_string();
        if let Some(u) = remove_row(&cur, &format!("~{txn}")) {
            self.as_mut().set_messages_json(QString::from(&u));
        }
    }

    /// 9.3: the local-echo engine. Registers the echo, paints its row into the timeline
    /// immediately (the whole point: what you typed is on screen before the network is),
    /// performs the real send, and stamps the row sent or failed. The row keeps its
    /// "~txn" key until the sync loop sees the real event in the window and drops the
    /// echo (load_room_data), so the QML delegate is touched exactly twice: once to
    /// un-dim it, once to swap in the server row.
    fn start_echo(mut self: Pin<&mut Self>, echo: Echo) {
        let room_id = self.room_id().to_string();
        if room_id.is_empty() {
            return;
        }
        // threads: a thread's panel sends into its thread (the fallback reply goes to its newest loaded event); a reply
        // keeps its parent's thread (A07.3)
        let mut echo = echo;
        // voice notes are sent from the main composer only (no relation helper on that path): never filed in a thread
        if !matches!(echo.kind, EchoKind::Voice(_)) {
            let rows = self.messages_json().to_string();
            echo.thread = self.thread_root().to_string();
            if !echo.thread.is_empty() {
                echo.thread_latest = newest_server_row(&rows).unwrap_or_else(|| echo.thread.clone());
            }
            if !echo.reply_to.is_empty() {
                echo.reply_thread = row_thread(&rows, &echo.reply_to);
            }
        }
        echo_store_push(&room_id, echo.clone());
        let me = crate::session::client()
            .and_then(|c| c.user_id().map(|u| u.to_string()))
            .unwrap_or_default();
        let my_name = self.rust().own_name.clone();
        let my_avatar = self.rust().own_avatar.clone();
        let my_name = if my_name.is_empty() {
            me.trim_start_matches('@').split(':').next().unwrap_or("me").to_string()
        } else {
            my_name
        };
        let cur = self.messages_json().to_string();
        let row = echo_row(&echo, &me, &my_name, &my_avatar, &cur);
        let is_dice = matches!(echo.kind, EchoKind::Dice(_));
        let is_poll = matches!(echo.kind, EchoKind::Poll(_));
        let next = upsert_row(&cur, row);
        if is_dice {
            self.as_mut().set_dice_json(QString::from(&dice_rows_json(&next)));
        }
        if is_poll {
            self.as_mut().set_polls_json(QString::from(&poll_rows_json(&next)));
        }
        self.as_mut().set_messages_json(QString::from(&next));
        let qt = self.qt_thread();
        let is_voice = matches!(echo.kind, EchoKind::Voice(_));
        // 4-3: the send goes through the client of the session that made it, never a later one
        let client = crate::session::client();
        crate::bridge::runtime().spawn(async move {
            let res = match &client {
                Some(c) => perform_echo(c, &room_id, &echo).await,
                None => Err("Not signed in".to_string()),
            };
            let txn = echo.txn.clone();
            if is_voice {
                // voice notes: the recorder row learns the outcome (sent → the draft is done; failed → review + retry)
                crate::voicenote_ctl::report_send(&txn, res.clone().map_err(|e| friendly_send_error(&e)));
            }
            let key = format!("~{txn}");
            let _ = qt.queue(move |mut o| match res {
                Ok(eid) => {
                    echo_store_sent(&room_id, &txn, &eid);
                    if o.room_id().to_string() == room_id {
                        let cur = o.messages_json().to_string();
                        if let Some(u) = patch_row(&cur, &key, |m| {
                            m["pending"] = serde_json::json!(false);
                            m["sentId"] = serde_json::json!(eid);
                            if is_voice {
                                // privacy: the local recording is deleted on success — the row shows the server's copy
                                m["media"] = serde_json::json!("");
                            }
                        }) {
                            if is_dice {
                                o.as_mut().set_dice_json(QString::from(&dice_rows_json(&u)));
                            }
                            if is_poll {
                                o.as_mut().set_polls_json(QString::from(&poll_rows_json(&u)));
                            }
                            o.as_mut().set_messages_json(QString::from(&u));
                        }
                    }
                }
                Err(e) => {
                    eprintln!("[room] send failed ({txn}): {e}");
                    echo_store_failed(&room_id, &txn, &e);
                    if o.room_id().to_string() == room_id {
                        let cur = o.messages_json().to_string();
                        if let Some(u) = patch_row(&cur, &key, |m| {
                            m["pending"] = serde_json::json!(false);
                            m["failed"] = serde_json::json!(true);
                            m["error"] = serde_json::json!(friendly_send_error(&e));
                        }) {
                            if is_dice {
                                o.as_mut().set_dice_json(QString::from(&dice_rows_json(&u)));
                            }
                            if is_poll {
                                o.as_mut().set_polls_json(QString::from(&poll_rows_json(&u)));
                            }
                            o.as_mut().set_messages_json(QString::from(&u));
                        }
                    }
                }
            });
        });
    }

    pub fn react(mut self: Pin<&mut Self>, event_id: &QString, key: &QString) {
        let room_id = self.room_id().to_string();
        let eid = event_id.to_string();
        let key = key.to_string();
        // Optimistic chip: reflect the reaction on the target message immediately so it
        // appears instantly, then reconcile on the next sync (which fully rebuilds the
        // reactions from server events — see load_room_data, so no double-count). The
        // optimistic chip carries the OPTIMISTIC_RID sentinel so we can roll it back on
        // a send failure and so unreact knows not to try to redact it on the server.
        let prev = self.messages_json().to_string();
        if let Some(updated) = apply_optimistic_react(&prev, &eid, &key, true) {
            self.as_mut().set_messages_json(QString::from(&updated));
        }
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            if let Err(e) = do_react(&room_id, &eid, &key).await {
                eprintln!("[room] react failed: {e}");
                // Roll back our optimistic chip (remove our just-added reaction).
                let _ = qt.queue(move |mut o| {
                    let cur = o.messages_json().to_string();
                    if let Some(rolled) = apply_optimistic_react(&cur, &eid, &key, false) {
                        o.as_mut().set_messages_json(QString::from(&rolled));
                    }
                });
            }
        });
    }

    pub fn unreact(mut self: Pin<&mut Self>, reaction_id: &QString) {
        let room_id = self.room_id().to_string();
        let rid = reaction_id.to_string();
        // Optimistic removal: drop OUR reaction from its chip immediately. We locate the
        // chip by its reactionId, capturing its (target_event_id, key) so a failed redact
        // can re-add it.
        let prev = self.messages_json().to_string();
        let rollback = remove_optimistic_react(&prev, &rid).map(|(updated, target, key)| {
            self.as_mut().set_messages_json(QString::from(&updated));
            (target, key)
        });
        // A purely-optimistic chip (never confirmed by the server) has no real redactable
        // event id — its removal above is the whole operation; don't hit the server.
        if rid == OPTIMISTIC_RID {
            return;
        }
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            if let Err(e) = do_unreact(&room_id, &rid).await {
                eprintln!("[room] unreact failed: {e}");
                // Roll back: re-add our reaction (optimistically) on its target message.
                if let Some((target, key)) = rollback {
                    let _ = qt.queue(move |mut o| {
                        let cur = o.messages_json().to_string();
                        if let Some(re) = apply_optimistic_react(&cur, &target, &key, true) {
                            o.as_mut().set_messages_json(QString::from(&re));
                        }
                    });
                }
            }
        });
    }

    pub fn send_with_mentions(mut self: Pin<&mut Self>, text: &QString, mentions_json: &QString) {
        let text = text.to_string();
        let room_id = self.room_id().to_string();
        if text.trim().is_empty() || room_id.is_empty() {
            return;
        }
        // [{name,userId}] from the composer's autocomplete picks.
        let mentions: Vec<(String, String)> =
            serde_json::from_str::<Vec<serde_json::Value>>(&mentions_json.to_string())
                .unwrap_or_default()
                .into_iter()
                .filter_map(|v| {
                    let name = v.get("name")?.as_str()?.to_string();
                    let uid = v.get("userId")?.as_str()?.to_string();
                    Some((name, uid))
                })
                .collect();
        self.as_mut().set_status(QString::default());
        let _ = room_id;
        self.as_mut().start_echo(Echo::new(EchoKind::Mentions(mentions), &text));
    }

    pub fn send_reply(mut self: Pin<&mut Self>, parent_id: &QString, text: &QString) {
        let room_id = self.room_id().to_string();
        let pid = parent_id.to_string();
        let text = text.to_string();
        if text.trim().is_empty() || room_id.is_empty() {
            return;
        }
        self.as_mut().set_status(QString::default());
        let _ = room_id;
        let mut echo = Echo::new(EchoKind::Reply(Vec::new()), &text);
        echo.reply_sender = row_sender(&self.messages_json().to_string(), &pid);
        echo.reply_to = pid;
        self.as_mut().start_echo(echo);
    }

    pub fn send_reply_mentions(mut self: Pin<&mut Self>, parent_id: &QString, text: &QString, mentions_json: &QString) {
        let room_id = self.room_id().to_string();
        let pid = parent_id.to_string();
        let text = text.to_string();
        if text.trim().is_empty() || room_id.is_empty() {
            return;
        }
        let mentions: Vec<(String, String)> =
            serde_json::from_str::<Vec<serde_json::Value>>(&mentions_json.to_string())
                .unwrap_or_default()
                .into_iter()
                .filter_map(|v| {
                    let name = v.get("name")?.as_str()?.to_string();
                    let uid = v.get("userId")?.as_str()?.to_string();
                    Some((name, uid))
                })
                .collect();
        self.as_mut().set_status(QString::default());
        let mut echo = Echo::new(EchoKind::Reply(mentions), &text);
        echo.reply_sender = row_sender(&self.messages_json().to_string(), &pid);
        echo.reply_to = pid;
        self.as_mut().start_echo(echo);
    }

    pub fn set_game_code(mut self: Pin<&mut Self>, game: &QString, code: &QString) {
        let room_id = self.room_id().to_string();
        let game = game.to_string();
        let code = code.to_string();
        if room_id.is_empty() {
            return;
        }
        // Optimistic local update for instant feedback; the live sync re-confirms (or a
        // failed send triggers a refresh that reverts to the server's truth).
        let optimistic = if code.trim().is_empty() {
            String::new()
        } else {
            serde_json::json!({ "game": game, "code": code, "sender": "" }).to_string()
        };
        self.as_mut().set_game_code_json(QString::from(&optimistic));
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            if let Err(e) = do_send_game_code(&room_id, &game, &code).await {
                eprintln!("[room] set game code failed: {e}");
                let _ = qt.queue(|mut o| o.as_mut().refresh());
            }
        });
    }

    pub fn edit_message(self: Pin<&mut Self>, event_id: &QString, text: &QString) {
        self.edit_message_mentions(event_id, text, &QString::from("[]"));
    }

    pub fn edit_message_mentions(mut self: Pin<&mut Self>, event_id: &QString, text: &QString, mentions_json: &QString) {
        let picks = parse_picks(&mentions_json.to_string());
        let room_id = self.room_id().to_string();
        let eid = event_id.to_string();
        let text = text.to_string();
        if text.trim().is_empty() || room_id.is_empty() {
            return;
        }
        let qt = self.qt_thread();
        self.as_mut().set_status(QString::default());
        // 9.3: the edit shows immediately (plain text until the sync brings the rendered
        // markdown); a failed edit reloads the server's truth.
        let cur = self.messages_json().to_string();
        // L1 A09.2: an edited /me stays an emote (the row's msgtype before the edit)
        let emote = row_msgtype(&cur, &eid).as_deref() == Some("m.emote");
        // L1 A09.3: the original's mention links that are still in the text, plus the picks made while editing
        let formatted = row_str(&cur, &eid, "formatted").unwrap_or_default();
        let mentions = edit_mentions(&formatted, &text, &picks);
        let notify = edit_notify_ids(&formatted, &mentions);
        let t2 = text.clone();
        if let Some(u) = patch_row(&cur, &eid, |m| {
            m["body"] = serde_json::json!(t2);
            m["formatted"] = serde_json::json!("");
            m["edited"] = serde_json::json!(true);
            m["embeds"] = serde_json::json!(crate::embeds::extract_embeds(&t2));
            m["onlyEmbed"] = serde_json::json!(crate::embeds::body_is_only_embed(&t2));
        }) {
            self.as_mut().set_messages_json(QString::from(&u));
        }
        crate::bridge::runtime().spawn(async move {
            if let Err(e) = do_edit(&room_id, &eid, &text, emote, &mentions, &notify).await {
                eprintln!("[room] edit failed: {e}");
                // L1 A20.0: no raw SDK error on screen; the view says it in plain words and puts the text back
                let j = op_failed(&room_id, "edit", &eid, &text, &e);
                let _ = qt.queue(move |mut o| {
                    let list = push_failure(&o.op_failed_json().to_string(), &j);
                    o.as_mut().set_op_failed_json(QString::from(&list));
                    o.as_mut().refresh();
                });
            }
        });
    }

    pub fn redact_message(mut self: Pin<&mut Self>, event_id: &QString) {
        let room_id = self.room_id().to_string();
        let eid = event_id.to_string();
        let qt = self.qt_thread();
        // 9.3: optimistic — the bubble turns into "Message deleted" now; a failed redact
        // reloads the server's truth.
        let cur = self.messages_json().to_string();
        if let Some(u) = patch_row(&cur, &eid, |m| {
            m["redacted"] = serde_json::json!(true);
            m["pinned"] = serde_json::json!(false); // QA W1: a deleted message carries no 📌
        }) {
            self.as_mut().set_messages_json(QString::from(&u));
        }
        crate::bridge::runtime().spawn(async move {
            if let Err(e) = do_redact(&room_id, &eid).await {
                eprintln!("[room] redact failed: {e}");
                let j = op_failed(&room_id, "delete", &eid, "", &e);
                let _ = qt.queue(move |mut o| {
                    let list = push_failure(&o.op_failed_json().to_string(), &j);
                    o.as_mut().set_op_failed_json(QString::from(&list));
                    o.as_mut().refresh();
                });
                return;
            }
            // QA W1: deleting a pinned message also unpins it, when I may edit the room's pins (pins::pins_after_delete;
            // a pin I may not take back is only hidden). After the delete went through, so a failed delete never loses
            // the pin.
            let Ok(room) = room_by_id(&room_id).await else { return };
            let may = load_permissions(&room).await.can_pin;
            // the list as it is right before the write (not before the permission read's await)
            let pins: Vec<String> = room.pinned_event_ids().unwrap_or_default().iter().map(|e| e.to_string()).collect();
            if let Some(ids) = crate::pins::pins_after_delete(&pins, &eid, may) {
                let res = room
                    .send_state_event_raw("m.room.pinned_events", "", serde_json::json!({ "pinned": ids }))
                    .await
                    .map(|_| ())
                    .map_err(|e| e.to_string());
                let _ = qt.queue(move |mut o| match res {
                    Ok(_) => o.as_mut().refresh(),
                    Err(e) => {
                        eprintln!("[room] unpin after delete failed: {e}");
                        o.as_mut().set_status(QString::from(&format!("Couldn't unpin that message: {}", mod_err("pin", &e))));
                    }
                });
            }
        });
    }

    /// 9.3: pin / unpin a message (m.room.pinned_events, whole-list write).
    /// The room's shared background (docs/AMBIENCE.md): one state event, written as the picker built it.
    pub fn set_background(self: Pin<&mut Self>, content_json: &QString) {
        let room_id = self.room_id().to_string();
        let Ok(content) = serde_json::from_str::<serde_json::Value>(&content_json.to_string()) else { return };
        if room_id.is_empty() || !content.is_object() {
            return;
        }
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let res: Result<(), String> = async {
                let room = room_by_id(&room_id).await?;
                crate::appstate::send_state(&room, tinhat_widgets::ambience::BACKGROUND_STATE, "", content.clone())
                    .await
                    .map_err(|e| e.to_string())?;
                eprintln!("[ambience] background set: {} {}", content.get("kind").and_then(|v| v.as_str()).unwrap_or(""), content.get("preset").and_then(|v| v.as_str()).unwrap_or(""));
                Ok(())
            }
            .await;
            let _ = qt.queue(move |mut o| match res {
                Ok(()) => o.as_mut().refresh(),
                Err(e) => {
                    eprintln!("[ambience] background write failed: {e}");
                    o.as_mut().set_status(QString::from("Couldn't change the background"));
                }
            });
        });
    }

    pub fn post_background_image(self: Pin<&mut Self>, file_url: &QString, preset: &QString, effect: &QString) {
        use tinhat_widgets::ambience as amb;
        let room_id = self.room_id().to_string();
        let src = crate::platform::path_from_file_url(&file_url.to_string());
        let (preset, effect) = (preset.to_string(), effect.to_string());
        if room_id.is_empty() || src.is_empty() {
            return;
        }
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let res: Result<String, String> = async {
                let dst = media_cache_dir().join(format!("chat-background-{}.jpg", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)));
                let (_reencoded, w, h) = tokio::task::spawn_blocking({
                    let (src, dst) = (src.clone(), dst.clone());
                    move || ambience_reencode(&src, &dst)
                })
                .await
                .map_err(|e| e.to_string())??;
                let room = room_by_id(&room_id).await?;
                if room.state() != matrix_sdk::RoomState::Joined {
                    return Err("not joined".into());
                }
                let data = tokio::fs::read(&dst).await.map_err(|e| e.to_string())?;
                // posted in the chat (PRODUCT-CALLS #40d): like every picture sent, it goes up without its location and
                // identifying metadata (the re-encode is Qt's; this makes it the shared rule's, not Qt's behaviour)
                let data = photo_bytes_for_upload("image/jpeg", data).map_err(|_| PHOTO_LOCATION_FAIL.to_string())?;
                let size = data.len() as u64;
                let client = room.client();
                let mime = mime::IMAGE_JPEG;
                let source = attach_upload_checked(
                    || async { room.latest_encryption_state().await.map_err(|x| x.to_string()) },
                    || room.encryption_state(),
                    |encrypted| {
                        let (client, data, mime) = (&client, &data, &mime);
                        async move {
                            use matrix_sdk::ruma::events::room::MediaSource;
                            if encrypted {
                                let mut cur = std::io::Cursor::new(data.as_slice());
                                let f = client.upload_encrypted_file(&mut cur).await.map_err(|x| x.to_string())?;
                                Ok(MediaSource::Encrypted(Box::new(f)))
                            } else {
                                let r = client.media().upload(mime, data.clone(), None).await.map_err(|x| x.to_string())?;
                                Ok(MediaSource::Plain(r.content_uri))
                            }
                        }
                    },
                )
                .await?;
                let content = attachment_content(&mime, "Chat background.jpg".into(), source, w, h, size);
                let mut content = serde_json::to_value(content).map_err(|x| x.to_string())?;
                content["body"] = serde_json::Value::from("Chat background");
                let r = room.send_raw("m.room.message", content).await.map_err(|x| x.to_string())?;
                let eid = r.response.event_id.to_string();
                let state = amb::background_content(amb::BackgroundKind::Image, Some(preset.as_str()).filter(|p| !p.is_empty()), amb::Effect::parse(&effect), Some(eid.as_str()));
                crate::appstate::send_state(&room, amb::BACKGROUND_STATE, "", state).await.map_err(|e| e.to_string())?;
                eprintln!("[ambience] background image posted ({w}x{h}, {size} B) and set");
                Ok(eid)
            }
            .await;
            let _ = qt.queue(move |mut o| match res {
                Ok(_) => o.as_mut().refresh(),
                Err(e) => {
                    eprintln!("[ambience] background image failed: {e}");
                    o.as_mut().set_status(QString::from("Couldn't post the image"));
                }
            });
        });
    }

    pub fn pin_message(self: Pin<&mut Self>, event_id: &QString, pinned: bool) {
        let room_id = self.room_id().to_string();
        let eid = event_id.to_string();
        if room_id.is_empty() || eid.is_empty() {
            return;
        }
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let res: Result<(), String> = async {
                let room = room_by_id(&room_id).await?;
                let mut ids: Vec<String> = room
                    .pinned_event_ids()
                    .unwrap_or_default()
                    .iter()
                    .map(|e| e.to_string())
                    .collect();
                if pinned {
                    if !ids.contains(&eid) {
                        ids.push(eid.clone());
                    }
                } else {
                    ids.retain(|i| i != &eid);
                }
                room.send_state_event_raw("m.room.pinned_events", "", serde_json::json!({ "pinned": ids }))
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(())
            }
            .await;
            let _ = qt.queue(move |mut o| match res {
                Ok(()) => o.as_mut().refresh(),
                Err(e) => {
                    eprintln!("[room] pin failed: {e}");
                    o.as_mut().set_status(QString::from(&format!(
                        "Couldn't {} that message: {}",
                        if pinned { "pin" } else { "unpin" },
                        mod_err("pin", &e)
                    )));
                }
            });
        });
    }

    // ---- Room-settings editing + moderation ---------------------------------
    // Every async block runs on bridge::runtime() (a bare tokio::spawn off the
    // bridge runtime PANICS). Failures surface via the mod_error qproperty
    // (mirrored to a passive toast in Main.qml, like activityCtl.lastError).

    /// Rename the room (m.room.name). Optimistic: update room_name + summary now,
    /// reconcile on the next sync.
    pub fn apply_room_name(mut self: Pin<&mut Self>, name: &QString) {
        let room_id = self.room_id().to_string();
        let name = name.to_string();
        if room_id.is_empty() {
            return;
        }
        self.as_mut().set_mod_error(QString::default());
        // Optimistic: show the new name in the header + summary immediately.
        self.as_mut().set_room_name(QString::from(&name));
        if let Some(updated) = patch_summary(&self.room_summary_json().to_string(), "name", &name) {
            self.as_mut().set_room_summary_json(QString::from(&updated));
        }
        let qt = self.qt_thread();
        let started = room_id.clone();
        crate::bridge::runtime().spawn(async move {
            if let Err(e) = do_set_name(&room_id, &name).await {
                eprintln!("[room] set_name failed: {e}");
                let _ = qt.queue(move |mut o| {
                    if o.room_id().to_string() == started {
                        o.as_mut().set_mod_error(QString::from(&mod_err("rename this room", &e)));
                    }
                });
            }
        });
    }

    /// Set the room topic (m.room.topic). Optimistic summary patch + sync reconcile.
    pub fn apply_room_topic(mut self: Pin<&mut Self>, topic: &QString) {
        let room_id = self.room_id().to_string();
        let topic = topic.to_string();
        if room_id.is_empty() {
            return;
        }
        self.as_mut().set_mod_error(QString::default());
        if let Some(updated) = patch_summary(&self.room_summary_json().to_string(), "topic", &topic) {
            self.as_mut().set_room_summary_json(QString::from(&updated));
        }
        let qt = self.qt_thread();
        let started = room_id.clone();
        crate::bridge::runtime().spawn(async move {
            if let Err(e) = do_set_topic(&room_id, &topic).await {
                eprintln!("[room] set_topic failed: {e}");
                let _ = qt.queue(move |mut o| {
                    if o.room_id().to_string() == started {
                        o.as_mut().set_mod_error(QString::from(&mod_err("set the topic", &e)));
                    }
                });
            }
        });
    }

    /// Drop a clip bookmark at NOW (0.8.8). State event, state_key = epoch-ms.
    pub fn clip_moment(self: Pin<&mut Self>) {
        let rid = self.room_id().clone();
        self.clip_moment_in(&rid)
    }

    /// 11.0: a clip bookmark in any joined room (the call's room — Rec / Clip act on the call you're in).
    pub fn clip_moment_in(self: Pin<&mut Self>, room_id: &QString) {
        let Some(client) = crate::session::client() else { return };
        let rid = room_id.to_string();
        if rid.is_empty() {
            return;
        }
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let res = async {
                let id = matrix_sdk::ruma::RoomId::parse(&rid).map_err(|e| e.to_string())?;
                let room = client.get_room(&id).ok_or_else(|| "room not found".to_string())?;
                let at = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                room.send_state_event_raw(
                    crate::appstate::CLIP_TYPE,
                    &at.to_string(),
                    serde_json::json!({ "at": at }),
                )
                .await
                .map_err(|e| e.to_string())?;
                Ok::<(), String>(())
            }
            .await;
            if let Err(e) = res {
                eprintln!("[memories] clip failed: {e}");
                let _ = qt.queue(move |mut o| {
                    o.as_mut().set_status(QString::from(&format!("Couldn't clip: {e}")));
                });
            }
        });
    }

    /// 11.0: read a room's recording marker into callRecordOn (the call's room; "" clears it) — the same read the open
    /// room gets at open() time; our own setRecordCallIn updates it after a write.
    pub fn refresh_call_record(mut self: Pin<&mut Self>, room_id: &QString) {
        let rid = room_id.to_string();
        self.as_mut().set_call_record_room(room_id.clone());
        self.as_mut().set_call_record_on(false);
        if rid.is_empty() {
            return;
        }
        let qt = self.as_mut().qt_thread();
        crate::bridge::runtime().spawn(async move {
            let on = async {
                let client = crate::session::client()?;
                let id = matrix_sdk::ruma::RoomId::parse(&rid).ok()?;
                let room = client.get_room(&id)?;
                crate::appstate::record_enabled(&room).await
            }
            .await
            .is_some();
            let _ = qt.queue(move |mut o| {
                if o.call_record_room().to_string() == rid {
                    o.as_mut().set_call_record_on(on)
                }
            });
        });
    }

    /// Load this room's DVR sessions from the server's vods API (0.8.8).
    pub fn fetch_memories(self: Pin<&mut Self>) {
        let rid = self.room_id().clone();
        self.fetch_memories_for(&rid)
    }

    /// 10.1 (TH-MEM-09): the list a room asks for need not be the room the main window has
    /// open — the Memories window remembers which room it was opened for.
    pub fn fetch_memories_for(self: Pin<&mut Self>, room_id: &QString) {
        let Some(client) = crate::session::client() else { return };
        let rid = room_id.to_string();
        if rid.is_empty() {
            return;
        }
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let token = client.access_token().unwrap_or_default();
            let enc = rid.replace('%', "%25").replace('!', "%21").replace(':', "%3A");
            match crate::proxy::get_json_auth(&format!("/vods/sessions?room={enc}"), &token).await {
                Ok(mut v) => {
                    // 10.5.1: the server signs the file links for whichever public name it was told
                    // (its former public name until its config moves); fetch them from our base.
                    crate::proxy::rebase_urls_in_json(&mut v);
                    let json = v.to_string();
                    let _ = qt.queue(move |mut o| o.as_mut().set_memories_json(QString::from(&json)));
                }
                Err(e) => {
                    // P9 (0.8.15): keep whatever list we already have — a 429/timeout
                    // used to blank the window. Only a never-loaded list gets the
                    // error payload (so the UI can say "couldn't load" instead of
                    // "no memories yet"), and it carries the sessions the QML expects.
                    eprintln!("[memories] fetch failed: {e}");
                    let err = serde_json::json!({ "sessions": [], "error": e }).to_string();
                    let _ = qt.queue(move |mut o| {
                        if o.as_ref().memories_json().is_empty() {
                            o.as_mut().set_memories_json(QString::from(&err));
                        }
                    });
                }
            }
        });
    }

    /// Memories photos: every load takes a ticket; only the newest load's answer (of this session) is published, so a
    /// slow answer for room A never replaces room B's, and an answer for an account that signed out is dropped.
    fn memory_photos_ticket() -> u64 {
        static T: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        T.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1
    }
    fn memory_photos_latest() -> &'static std::sync::atomic::AtomicU64 {
        static L: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        &L
    }

    /// Memories photos (memories_photos.rs): load that room's photos into memoryPhotosJson.
    pub fn fetch_memory_photos(self: Pin<&mut Self>, room_id: &QString) {
        let Some(client) = crate::session::client() else { return };
        let rid = room_id.to_string();
        let Ok(id) = matrix_sdk::ruma::RoomId::parse(&rid) else { return };
        let qt = self.qt_thread();
        let gen = crate::session::generation();
        let ticket = Self::memory_photos_ticket();
        Self::memory_photos_latest().store(ticket, std::sync::atomic::Ordering::SeqCst);
        crate::bridge::runtime().spawn(async move {
            let Some(room) = client.get_room(&id) else { return };
            let json = crate::memories_photos::load(&room).await.to_string();
            let _ = qt.queue(move |mut o| {
                if gen == crate::session::generation() && Self::memory_photos_latest().load(std::sync::atomic::Ordering::SeqCst) == ticket {
                    o.as_mut().set_memory_photos_json(QString::from(&json));
                }
            });
        });
    }

    /// One photo write, then the words for a toast and a fresh memoryPhotosJson.
    fn memory_photo_op<F, Fut>(self: Pin<&mut Self>, room_id: &QString, ok_words: &'static str, op: F)
    where
        F: FnOnce(matrix_sdk::Room) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<(), String>> + Send,
    {
        let Some(client) = crate::session::client() else { return };
        let rid = room_id.to_string();
        let Ok(id) = matrix_sdk::ruma::RoomId::parse(&rid) else { return };
        let qt = self.qt_thread();
        let gen = crate::session::generation();
        crate::bridge::runtime().spawn(async move {
            let Some(room) = client.get_room(&id) else { return };
            let words = match op(room.clone()).await {
                Ok(()) => ok_words.to_string(),
                Err(e) => e,
            };
            let ticket = Self::memory_photos_ticket();
            Self::memory_photos_latest().store(ticket, std::sync::atomic::Ordering::SeqCst);
            let json = crate::memories_photos::load(&room).await.to_string();
            let _ = qt.queue(move |mut o| {
                if gen != crate::session::generation() {
                    return; // signed out meanwhile: nothing of that account is shown
                }
                let n = o.as_ref().memory_photos_notice().to_string().split('|').next().and_then(|x| x.parse::<u64>().ok()).unwrap_or(0) + 1;
                o.as_mut().set_memory_photos_notice(QString::from(&format!("{n}|{words}")));
                if Self::memory_photos_latest().load(std::sync::atomic::Ordering::SeqCst) == ticket {
                    o.as_mut().set_memory_photos_json(QString::from(&json));
                }
            });
        });
    }

    pub fn add_to_memories(self: Pin<&mut Self>, room_id: &QString, event_id: &QString, ts: f64) {
        let eid = event_id.to_string();
        self.memory_photo_op(room_id, tinhat_widgets::memphotos::ADDED_TOAST, move |room| async move { crate::memories_photos::add(&room, &eid, ts).await });
    }

    pub fn remove_from_memories(self: Pin<&mut Self>, room_id: &QString, event_id: &QString, also_delete: bool) {
        let eid = event_id.to_string();
        let words = if also_delete { "Removed from Memories and the chat" } else { tinhat_widgets::memphotos::REMOVED_TOAST };
        self.memory_photo_op(room_id, words, move |room| async move {
            crate::memories_photos::remove(&room, &eid).await?;
            if also_delete {
                let e = matrix_sdk::ruma::EventId::parse(&eid).map_err(|e| e.to_string())?;
                room.redact(&e, None, None).await.map_err(|e| format!("Couldn't delete it from the chat ({e})"))?;
                crate::memories_photos::forget(room.room_id().as_str(), &eid);
            }
            Ok(())
        });
    }

    pub fn take_down_memory(self: Pin<&mut Self>, room_id: &QString, event_id: &QString) {
        let eid = event_id.to_string();
        self.memory_photo_op(room_id, tinhat_widgets::memphotos::REMOVED_TOAST, move |room| async move { crate::memories_photos::take_down(&room, &eid).await });
    }

    pub fn delete_memory_photo(self: Pin<&mut Self>, room_id: &QString, event_id: &QString) {
        let eid = event_id.to_string();
        self.memory_photo_op(room_id, tinhat_widgets::memphotos::DELETED_TOAST, move |room| async move {
            let e = matrix_sdk::ruma::EventId::parse(&eid).map_err(|e| e.to_string())?;
            room.redact(&e, None, None).await.map_err(|e| format!("Couldn't delete it ({e})"))?;
            crate::memories_photos::forget(room.room_id().as_str(), &eid);
            Ok(())
        });
    }

    pub fn add_memory_photos(self: Pin<&mut Self>, room_id: &QString, urls_json: &QString) {
        let urls: Vec<String> = serde_json::from_str(&urls_json.to_string()).unwrap_or_default();
        if urls.is_empty() {
            return;
        }
        self.memory_photo_op(room_id, tinhat_widgets::memphotos::ADDED_TOAST, move |room| async move {
            let mut first_err: Option<String> = None;
            for u in urls {
                let path = crate::memories_photos::local_path(&u);
                if let Err(e) = crate::memories_photos::send_photo(&room, &path).await {
                    eprintln!("[memories] add photo failed: {e}");
                    first_err.get_or_insert(e);
                }
            }
            first_err.map_or(Ok(()), Err)
        });
    }

    pub fn hidden_in_chat(&self, room_id: &QString, event_id: &QString) -> bool {
        crate::memories_photos::hidden_in_chat(&room_id.to_string(), &event_id.to_string())
    }

    pub fn in_my_memories(&self, room_id: &QString, event_id: &QString) -> bool {
        let (rid, eid) = (room_id.to_string(), event_id.to_string());
        let known = crate::memories_photos::in_my_memories(&rid, &eid);
        // read my list in the background so the next menu is current (a room not read yet answers false)
        if let (Some(client), Ok(id)) = (crate::session::client(), matrix_sdk::ruma::RoomId::parse(&rid)) {
            crate::bridge::runtime().spawn(async move {
                if let Some(room) = client.get_room(&id) {
                    crate::memories_photos::refresh_mine(&room).await;
                }
            });
        }
        known
    }

    pub fn memories_timeline(&self, sessions_json: &QString, photos_json: &QString, filter: &QString) -> QString {
        use tinhat_widgets::memphotos as mp;
        let sessions: serde_json::Value = serde_json::from_str(&sessions_json.to_string()).unwrap_or(serde_json::Value::Null);
        let photos: serde_json::Value = serde_json::from_str(&photos_json.to_string()).unwrap_or(serde_json::Value::Null);
        let recs = sessions.as_array().cloned().unwrap_or_default();
        let phs = photos.as_array().cloned().unwrap_or_default();
        let items = mp::timeline(&recs, &phs, mp::Filter::parse(&filter.to_string()));
        QString::from(&serde_json::Value::Array(items.iter().map(mp::Item::to_json).collect()).to_string())
    }

    pub fn memories_step(&self, items_json: &QString, current: &QString, dir: i32) -> QString {
        use tinhat_widgets::memphotos as mp;
        let v: serde_json::Value = serde_json::from_str(&items_json.to_string()).unwrap_or(serde_json::Value::Null);
        let items: Vec<mp::Item> = v.as_array().map(|a| a.iter().filter_map(mp::Item::from_json).collect()).unwrap_or_default();
        QString::from(&mp::step(&items, &current.to_string(), if dir < 0 { -1 } else { 1 }))
    }

    pub fn save_memory_photo(self: Pin<&mut Self>, media_url: &QString, dest: &QString) {
        let src = crate::memories_photos::local_path(&media_url.to_string());
        let dest = crate::memories_photos::local_path(&dest.to_string());
        // only a file of the media cache (what memories_photos::load handed out), never another path
        let cache = media_cache_dir();
        let inside = src.canonicalize().ok().zip(cache.canonicalize().ok()).is_some_and(|(s, c)| s.starts_with(c));
        if dest.as_os_str().is_empty() || !inside {
            return;
        }
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let words = match tokio::fs::copy(&src, &dest).await {
                Ok(_) => "Saved".to_string(),
                Err(e) => format!("Couldn't save it ({e})"),
            };
            let _ = qt.queue(move |mut o| {
                let n = o.as_ref().memory_photos_notice().to_string().split('|').next().and_then(|x| x.parse::<u64>().ok()).unwrap_or(0) + 1;
                o.as_mut().set_memory_photos_notice(QString::from(&format!("{n}|{words}")));
            });
        });
    }

    pub fn memory_photo_exts(&self) -> QString {
        QString::from(&serde_json::to_string(crate::memories_photos::PHOTO_EXTS).unwrap_or_else(|_| "[]".into()))
    }

    /// Compiled only outside a release (cfg tinhat_env_selftests, build.rs): a release build never reads the variable,
    /// and scripts/release-binary-check.sh refuses a binary that names it (it uploads files: never in a real launch).
    pub fn memory_photos_gate(&self) -> QString {
        #[cfg(tinhat_env_selftests)]
        {
            QString::from(std::env::var("TINHAT_GUI_MEMPHOTOS").unwrap_or_default().as_str())
        }
        #[cfg(not(tinhat_env_selftests))]
        {
            QString::default()
        }
    }

    pub fn memories_window_mode(&self, recorder: bool) -> QString {
        QString::from(&tinhat_widgets::memphotos::window_mode(recorder).to_string())
    }

    pub fn memory_photos_rev(&self, room_id: &QString) -> i32 {
        (crate::memories_photos::rev_of(&room_id.to_string()) % (i32::MAX as u64)) as i32
    }

    pub fn memories_count_text(&self, recordings: i32, photos: i32) -> QString {
        QString::from(&tinhat_widgets::memphotos::count_text(recordings.max(0) as u64, photos.max(0) as u64))
    }

    /// Mirror clip bookmarks (state events) into clipsJson (0.8.8).
    pub fn fetch_clips(self: Pin<&mut Self>) {
        let rid = self.room_id().clone();
        self.fetch_clips_for(&rid)
    }

    /// 10.1 (TH-MEM-09)
    pub fn fetch_clips_for(self: Pin<&mut Self>, room_id: &QString) {
        let Some(client) = crate::session::client() else { return };
        let rid = room_id.to_string();
        if rid.is_empty() {
            return;
        }
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let json = async {
                let id = matrix_sdk::ruma::RoomId::parse(&rid).ok()?;
                let room = client.get_room(&id)?;
                let all = crate::appstate::get_state_all(&room, crate::appstate::CLIP_TYPE).await;
                let arr: Vec<serde_json::Value> = all
                    .iter()
                    .filter_map(|(_k, sender, v)| {
                        let at = v.get("at").and_then(|a| a.as_u64())?;
                        Some(serde_json::json!({ "at": at, "by": sender }))
                    })
                    .collect();
                serde_json::to_string(&arr).ok()
            }
            .await
            .unwrap_or_else(|| "[]".to_string());
            let _ = qt.queue(move |mut o| o.as_mut().set_clips_json(QString::from(&json)));
        });
    }

    /// Turn Memories call recording on/off for this room (0.8.7).
    ///
    /// ON: invite the recorder account (idempotent — an "already in the room" invite
    /// failure is success) and write the marker; the bot joins the call as a VISIBLE
    /// participant and records everyone who hasn't opted out ("Never record me" in
    /// Settings → Calls). OFF: clear the marker; the bot leaves and finalizes.
    pub fn set_record_call(self: Pin<&mut Self>, on: bool) {
        let rid = self.room_id().clone();
        self.set_record_call_in(&rid, on)
    }

    /// 11.0: set_record_call for any joined room (the call's room). The open room's recordOn and callRecordOn follow
    /// only when they describe that room (the open room may have changed while the write was in flight).
    pub fn set_record_call_in(mut self: Pin<&mut Self>, room_id: &QString, on: bool) {
        let Some(client) = crate::session::client() else { return };
        let rid = room_id.to_string();
        if rid.is_empty() {
            return;
        }
        self.as_mut().set_status(QString::from(if on {
            "Starting call recording…"
        } else {
            "Stopping call recording…"
        }));
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let msg = match do_set_record_call(&client, &rid, on).await {
                Ok(()) => {
                    let r2 = rid.clone();
                    let _ = qt.queue(move |mut o| {
                        if o.room_id().to_string() == r2 {
                            o.as_mut().set_record_on(on);
                        }
                        if o.call_record_room().to_string() == r2 {
                            o.as_mut().set_call_record_on(on);
                        }
                    });
                    if on {
                        "Recording on — the recorder joins the call when it starts".to_string()
                    } else {
                        "Recording stopped".to_string()
                    }
                }
                Err(e) => {
                    eprintln!("[vod] set_record_call failed: {e}");
                    format!("Couldn't change recording: {e}")
                }
            };
            let _ = qt.queue(move |mut o| o.as_mut().set_status(QString::from(&msg)));
        });
    }

    /// Memories LIVE side tiles — thin GUI-thread bridges to
    /// streamer::memtile + the video_bridge attach shim. memtile_create returning
    /// false (parse/plugin failure) flips that tile to the QML thumbnail fallback.
    pub fn memtile_create(self: Pin<&mut Self>, id: &QString, url: &QString) -> bool {
        // Slot reuse: create() retires any previous pipeline under this id, but
        // retire() cannot predetach (that's app-side) — unbind the qml6glsink widget
        // FIRST on the GUI thread, mirroring memtile_destroy's ordering (sink-widget
        // rule — video_bridge.cpp predetach docs).
        if let Some(ptr) = tinhat_streamer::memtile::pipeline_ptr(&id.to_string()) {
            crate::video::predetach(ptr);
        }
        match tinhat_streamer::memtile::create(&id.to_string(), &url.to_string()) {
            Ok(_) => true,
            Err(e) => {
                eprintln!("[memtile] create {id} failed (thumbnail fallback): {e:#}");
                false
            }
        }
    }

    pub fn memtile_attach(self: Pin<&mut Self>, id: &QString) -> bool {
        let id = id.to_string();
        let Some(ptr) = tinhat_streamer::memtile::pipeline_ptr(&id) else {
            return false;
        };
        crate::video::attach(ptr, &id)
    }

    pub fn memtile_sync(self: Pin<&mut Self>, id: &QString, secs: &QString, hard: bool) {
        if let Ok(s) = secs.to_string().parse::<f64>() {
            tinhat_streamer::memtile::sync_to(&id.to_string(), s, hard);
        }
    }

    pub fn memtile_set_paused(self: Pin<&mut Self>, id: &QString, paused: bool) {
        tinhat_streamer::memtile::set_paused(&id.to_string(), paused);
    }

    pub fn memtile_destroy(self: Pin<&mut Self>, id: &QString) {
        let id = id.to_string();
        if let Some(ptr) = tinhat_streamer::memtile::pipeline_ptr(&id) {
            crate::video::predetach(ptr); // GUI thread, BEFORE NULL — sink-widget rule
        }
        tinhat_streamer::memtile::retire(&id);
    }

    pub fn memtile_dead(self: Pin<&mut Self>, id: &QString) -> bool {
        tinhat_streamer::memtile::is_dead(&id.to_string())
    }

    pub fn memtile_frames(self: Pin<&mut Self>, id: &QString) -> i32 {
        tinhat_streamer::memtile::frame_count(&id.to_string()).clamp(-1, i32::MAX as i64) as i32
    }

    pub fn memtile_position(self: Pin<&mut Self>, id: &QString) -> QString {
        QString::from(&format!("{:.3}", tinhat_streamer::memtile::position(&id.to_string())))
    }

    pub fn memtile_probe_state(self: Pin<&mut Self>) -> i32 {
        tinhat_streamer::memtile::kick_probe();
        tinhat_streamer::memtile::probe_state()
    }

    pub fn memtile_clear(self: Pin<&mut Self>) {
        for id in tinhat_streamer::memtile::ids() {
            if let Some(ptr) = tinhat_streamer::memtile::pipeline_ptr(&id) {
                crate::video::predetach(ptr);
            }
            tinhat_streamer::memtile::retire(&id);
        }
    }

    /// Set (or with an empty path, remove) the room's picture.
    pub fn apply_room_photo(mut self: Pin<&mut Self>, path: &QString) {
        let Some(client) = crate::session::client() else { return };
        let rid = self.room_id().to_string();
        let path = path.to_string();
        if rid.is_empty() {
            return;
        }
        self.as_mut().set_status(QString::from(if path.is_empty() {
            "Removing photo…"
        } else {
            "Uploading photo…"
        }));
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let msg = match do_apply_room_photo(&client, &rid, &path).await {
                Ok(()) => String::new(),
                Err(e) => {
                    eprintln!("[room] apply_room_photo failed: {e}");
                    format!("Couldn't change the photo: {e}")
                }
            };
            let _ = qt.queue(move |mut o| o.as_mut().set_status(QString::from(&msg)));
        });
    }

    pub fn invite_user(mut self: Pin<&mut Self>, user_id: &QString) {
        let room_id = self.room_id().to_string();
        let uid = user_id.to_string();
        if room_id.is_empty() || uid.trim().is_empty() {
            return;
        }
        // 2026-11-12 (feat/people-search): a plain username means your own server (dm_target.rs, every picker)
        let uid = crate::people::own_target(&uid).unwrap_or(uid);
        self.as_mut().set_mod_error(QString::default());
        let qt = self.qt_thread();
        let started = room_id.clone();
        crate::bridge::runtime().spawn(async move {
            match do_invite(&room_id, &uid).await {
                Ok(()) => {
                    let _ = qt.queue(move |mut o| {
                        if o.room_id().to_string() == started {
                            o.as_mut().refresh();
                        }
                    });
                }
                Err(e) => {
                    eprintln!("[room] invite failed: {e}");
                    let _ = qt.queue(move |mut o| {
                        if o.room_id().to_string() == started {
                            o.as_mut().set_mod_error(QString::from(&mod_err("invite that user", &e)));
                        }
                    });
                }
            }
        });
    }

    pub fn kick_user(self: Pin<&mut Self>, user_id: &QString, reason: &QString) {
        self.moderate_member(user_id, reason, ModAction::Kick);
    }
    pub fn ban_user(self: Pin<&mut Self>, user_id: &QString, reason: &QString) {
        self.moderate_member(user_id, reason, ModAction::Ban);
    }
    pub fn unban_user(self: Pin<&mut Self>, user_id: &QString, reason: &QString) {
        self.moderate_member(user_id, reason, ModAction::Unban);
    }

    /// Shared kick/ban/unban: run the action, refresh on success, toast on failure.
    fn moderate_member(mut self: Pin<&mut Self>, user_id: &QString, reason: &QString, action: ModAction) {
        let room_id = self.room_id().to_string();
        let uid = user_id.to_string();
        let reason = reason.to_string();
        if room_id.is_empty() || uid.trim().is_empty() {
            return;
        }
        self.as_mut().set_mod_error(QString::default());
        let qt = self.qt_thread();
        let started = room_id.clone();
        crate::bridge::runtime().spawn(async move {
            let reason = if reason.trim().is_empty() { None } else { Some(reason.as_str()) };
            match do_moderate(&room_id, &uid, reason, action).await {
                Ok(()) => {
                    let _ = qt.queue(move |mut o| {
                        if o.room_id().to_string() == started {
                            o.as_mut().refresh();
                        }
                    });
                }
                Err(e) => {
                    eprintln!("[room] {} failed: {e}", action.verb());
                    let _ = qt.queue(move |mut o| {
                        if o.room_id().to_string() == started {
                            o.as_mut().set_mod_error(QString::from(&mod_err(action.verb(), &e)));
                        }
                    });
                }
            }
        });
    }

    /// Set a member's power level (m.room.power_levels). Reconciles on next sync.
    pub fn set_member_power(mut self: Pin<&mut Self>, user_id: &QString, power: i32) {
        let room_id = self.room_id().to_string();
        let uid = user_id.to_string();
        if room_id.is_empty() || uid.trim().is_empty() {
            return;
        }
        // 2026-11-12 (feat/people-search): a plain username means your own server (dm_target.rs, every picker)
        let uid = crate::people::own_target(&uid).unwrap_or(uid);
        self.as_mut().set_mod_error(QString::default());
        let qt = self.qt_thread();
        let started = room_id.clone();
        crate::bridge::runtime().spawn(async move {
            match do_set_power(&room_id, &uid, power).await {
                Ok(()) => {
                    let _ = qt.queue(move |mut o| {
                        if o.room_id().to_string() == started {
                            o.as_mut().refresh();
                        }
                    });
                }
                Err(e) => {
                    eprintln!("[room] set_power failed: {e}");
                    let _ = qt.queue(move |mut o| {
                        if o.room_id().to_string() == started {
                            o.as_mut().set_mod_error(QString::from(&mod_err("change their role", &e)));
                        }
                    });
                }
            }
        });
    }
}

/// Resolve a room by id (shared by the message-action helpers below).
async fn room_by_id(room_id: &str) -> Result<matrix_sdk::Room, String> {
    let client = crate::session::client().ok_or_else(|| "Not signed in".to_string())?;
    let rid = RoomId::parse(room_id).map_err(|e| format!("bad room id: {e}"))?;
    client.get_room(&rid).ok_or_else(|| "Room not found".to_string())
}

/// Post the current game's join code as a custom `dev.tinhat.game.code` timeline event.
/// Any member may send it (timeline events are power-level 0); the latest wins. An empty
/// `code` clears it. In E2EE rooms the SDK encrypts it on the wire; stock clients ignore the
/// unknown type (no chat bubble). No server-side code or room config needed.
async fn do_send_game_code(room_id: &str, game: &str, code: &str) -> Result<(), String> {
    let room = room_by_id(room_id).await?;
    let content = serde_json::json!({ "game": game, "code": code });
    room.send_raw("dev.tinhat.game.code", content)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Edit our message `target_id` via an `m.replace` relation + `m.new_content`.
async fn do_edit(room_id: &str, target_id: &str, text: &str, emote: bool, mentions: &[(String, String)], notify: &[String]) -> Result<(), String> {
    let room = room_by_id(room_id).await?;
    let content = edit_content(target_id, text, emote, mentions, notify)?;
    room.send_raw("m.room.message", content).await.map_err(|e| e.to_string())?;
    Ok(())
}

/// The edit's content: markdown(new text) as `m.new_content`, the same with "* " as the fallback body. L1 A09.2: an
/// emote's edit stays an emote (it always went out as m.text; Tin Hat Web's markdown.ts editContent does the same now).
/// L1 A09.3: with mentions, the body links them as a new message's picks do (render_mention_html + m.mentions); the top
/// level pings only `notify` (the newly mentioned, edit_notify_ids — the Matrix spec), m.new_content carries them all.
fn edit_content(target_id: &str, text: &str, emote: bool, mentions: &[(String, String)], notify: &[String]) -> Result<serde_json::Value, String> {
    let inner = if mentions.is_empty() {
        if emote { RoomMessageEventContent::emote_markdown(text) } else { RoomMessageEventContent::text_markdown(text) }
    } else {
        use matrix_sdk::ruma::{events::Mentions, OwnedUserId, UserId};
        let html = render_mention_html(text, mentions);
        let ids: Vec<OwnedUserId> = mentions.iter().filter_map(|(_, u)| UserId::parse(u).ok()).collect();
        let c = if emote { RoomMessageEventContent::emote_html(text, html) } else { RoomMessageEventContent::text_html(text, html) };
        if ids.is_empty() { c } else { c.add_mentions(Mentions::with_user_ids(ids)) }
    };
    let new_json = serde_json::to_value(inner).map_err(|e| e.to_string())?;
    let mut content = new_json.clone();
    content["body"] = serde_json::json!(format!("* {text}")); // fallback for old clients
    content["m.new_content"] = new_json;
    content["m.relates_to"] = serde_json::json!({ "rel_type": "m.replace", "event_id": target_id });
    if !mentions.is_empty() {
        content["m.mentions"] = if notify.is_empty() { serde_json::json!({}) } else { serde_json::json!({ "user_ids": notify }) };
    }
    Ok(content)
}

/// The msgtype of the row `id` in a timeline JSON (None = not there / none).
fn row_msgtype(json: &str, id: &str) -> Option<String> {
    row_str(json, id, "msgtype")
}

/// A string field of the row `id` in a timeline JSON.
fn row_str(json: &str, id: &str, field: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let m = v.as_array()?.iter().find(|m| m.get("id").and_then(|i| i.as_str()) == Some(id))?;
    m.get(field).and_then(|t| t.as_str()).map(str::to_string)
}

/// The composer's picks `[{name, userId}]`.
fn parse_picks(json: &str) -> Vec<(String, String)> {
    serde_json::from_str::<Vec<serde_json::Value>>(json)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| Some((v.get("name")?.as_str()?.to_string(), v.get("userId")?.as_str()?.to_string())))
        .collect()
}

/// L1 A09.3: the people a formatted body links (matrix.to/#/@user anchors): (name without its "@", user id).
fn linked_mentions(formatted: &str) -> Vec<(String, String)> {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r#"<a href="https://matrix\.to/#/([^"]+)">@([^<]*)</a>"#).unwrap());
    re.captures_iter(formatted)
        .filter_map(|c| {
            let uid = percent_decode_lossy(&c[1]);
            if !uid.starts_with('@') {
                return None;
            }
            let name = c[2].replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&#x27;", "'").replace("&amp;", "&");
            Some((name, uid))
        })
        .collect()
}

fn percent_decode_lossy(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = |c: u8| (c as char).to_digit(16);
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// L1 A09.3: the edit's top-level m.mentions: the sorted user ids among `mentions` the original did not link (the Matrix
/// spec: an edit pings only the newly mentioned). Tin Hat Web's editNotifyIds; testvectors/edit-mentions "notify".
fn edit_notify_ids(formatted: &str, mentions: &[(String, String)]) -> Vec<String> {
    let had: std::collections::BTreeSet<String> = linked_mentions(formatted).into_iter().map(|(_, u)| u).collect();
    let ids: std::collections::BTreeSet<String> = mentions.iter().map(|(_, u)| u.clone()).filter(|u| !had.contains(u)).collect();
    ids.into_iter().collect()
}

/// L1 A09.3: an edit's mentions (Tin Hat Web's editMentions; testvectors/edit-mentions): the picks made while editing,
/// then the people the original linked, each only while its "@name" is in the new text; one per user id, first wins.
fn edit_mentions(formatted: &str, text: &str, picks: &[(String, String)]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for (name, uid) in picks.iter().cloned().chain(linked_mentions(formatted)) {
        if out.iter().any(|(_, u)| *u == uid) || !text.contains(&format!("@{name}")) {
            continue;
        }
        out.push((name, uid));
    }
    out
}

#[cfg(test)]
mod edit_content_tests {
    #[test]
    fn edit_mentions_shared_vectors() {
        let v: serde_json::Value = serde_json::from_str(include_str!("../../../testvectors/edit-mentions/vectors.json")).unwrap();
        let pairs = |a: &serde_json::Value| -> Vec<(String, String)> {
            a.as_array().unwrap().iter().map(|m| (m["name"].as_str().unwrap().to_string(), m["userId"].as_str().unwrap().to_string())).collect()
        };
        let cases = v["cases"].as_array().unwrap();
        assert!(cases.len() >= 10);
        for c in cases {
            let got = super::edit_mentions(c["formatted"].as_str().unwrap(), c["text"].as_str().unwrap(), &pairs(&c["picks"]));
            assert_eq!(got, pairs(&c["mentions"]), "{}", c["name"]);
            let notify: Vec<String> = c["notify"].as_array().unwrap().iter().map(|u| u.as_str().unwrap().to_string()).collect();
            assert_eq!(super::edit_notify_ids(c["formatted"].as_str().unwrap(), &got), notify, "{}: notify", c["name"]);
        }
    }

    #[test]
    fn an_edit_with_mentions_links_them() {
        let m = vec![("Bo".to_string(), "@bo:x.org".to_string())];
        let e = super::edit_content("$orig", "hi @Bo", false, &m, &[]).unwrap();
        assert_eq!(e["m.mentions"], serde_json::json!({}), "an edit doesn't ping the already mentioned again");
        let n = &e["m.new_content"];
        assert_eq!(n["msgtype"], "m.text");
        assert!(n["formatted_body"].as_str().unwrap().contains(r#"<a href="https://matrix.to/#/@bo:x.org">@Bo</a>"#));
        assert_eq!(n["m.mentions"]["user_ids"][0], "@bo:x.org");
        let em = super::edit_content("$orig", "pokes @Bo", true, &m, &["@bo:x.org".to_string()]).unwrap();
        assert_eq!(em["m.new_content"]["msgtype"], "m.emote");
        assert_eq!(em["m.mentions"]["user_ids"][0], "@bo:x.org");
    }

    #[test]
    fn an_emote_edit_stays_an_emote() {
        let e = super::edit_content("$orig", "waves *hard*", true, &[], &[]).unwrap();
        assert_eq!(e["msgtype"], "m.emote");
        assert_eq!(e["m.new_content"]["msgtype"], "m.emote");
        assert_eq!(e["m.new_content"]["body"], "waves *hard*");
        assert_eq!(e["body"], "* waves *hard*");
        assert_eq!(e["m.relates_to"]["rel_type"], "m.replace");
        let t = super::edit_content("$orig", "plain", false, &[], &[]).unwrap();
        assert_eq!(t["m.new_content"]["msgtype"], "m.text");
    }

    #[test]
    fn the_rows_msgtype_decides() {
        let rows = r#"[{"id":"$a","msgtype":"m.emote","body":"waves"},{"id":"$b","msgtype":"m.text","body":"hi"}]"#;
        assert_eq!(super::row_msgtype(rows, "$a").as_deref(), Some("m.emote"));
        assert_eq!(super::row_msgtype(rows, "$b").as_deref(), Some("m.text"));
        assert_eq!(super::row_msgtype(rows, "$c"), None);
    }
}

/// Redact (delete) a message by event id.
async fn do_redact(room_id: &str, event_id: &str) -> Result<(), String> {
    let room = room_by_id(room_id).await?;
    let eid = matrix_sdk::ruma::EventId::parse(event_id).map_err(|e| format!("bad event id: {e}"))?;
    room.redact(&eid, None, None).await.map_err(|e| e.to_string())?;
    Ok(())
}

// ---- Room-settings editing + moderation helpers -----------------------------

/// Which kick/ban/unban action a moderate_member call performs.
#[derive(Clone, Copy)]
enum ModAction {
    Kick,
    Ban,
    Unban,
}
impl ModAction {
    fn verb(self) -> &'static str {
        match self {
            ModAction::Kick => "kick that member",
            ModAction::Ban => "ban that member",
            ModAction::Unban => "unban that user",
        }
    }
}

/// Build a friendly toast for a failed moderation/settings write. Most failures are a
/// permission (M_FORBIDDEN) — phrase for that, but keep the raw error for the log.
fn mod_err(action: &str, _raw: &str) -> String {
    format!("Couldn't {action} — you may not have permission in this room")
}

/// Patch a single string field of the room-summary JSON (optimistic name/topic edit).
/// Returns the new JSON, or None if the summary doesn't parse.
fn patch_summary(summary_json: &str, key: &str, value: &str) -> Option<String> {
    let mut v: serde_json::Value = serde_json::from_str(summary_json).ok()?;
    if !v.is_object() {
        return None;
    }
    v[key] = serde_json::json!(value);
    serde_json::to_string(&v).ok()
}

async fn do_set_name(room_id: &str, name: &str) -> Result<(), String> {
    let room = room_by_id(room_id).await?;
    room.set_name(name.to_string()).await.map_err(|e| e.to_string())?;
    Ok(())
}

async fn do_set_topic(room_id: &str, topic: &str) -> Result<(), String> {
    let room = room_by_id(room_id).await?;
    room.set_room_topic(topic).await.map_err(|e| e.to_string())?;
    Ok(())
}

/// Invite WITHOUT the room's history (2026-10-23, rooms are encrypted by default): matrix-sdk's `invite_user_by_id`
/// hands every invitee the keys to the room's earlier messages first (MSC4268 — what a person you invite should get).
/// An account that only records calls (the Memories recorder) must not get them: a plain /invite. The web's
/// core/matrix inviteWithoutHistory does the same.
pub(crate) async fn invite_without_history(
    room: &matrix_sdk::Room,
    user: &matrix_sdk::ruma::UserId,
) -> Result<(), matrix_sdk::Error> {
    use matrix_sdk::ruma::api::client::membership::invite_user::{
        self,
        v3::{InvitationRecipient, InviteUserId},
    };
    let req = invite_user::v3::Request::new(
        room.room_id().to_owned(),
        InvitationRecipient::UserId(InviteUserId::new(user.to_owned())),
    );
    room.client().send(req).await?;
    // what matrix-sdk's invite_user_by_id does after its POST: reload the members before the next encrypted send, so
    // a message sent before the next sync still shares its key with the invitee's devices
    room.mark_members_missing();
    Ok(())
}

async fn do_invite(room_id: &str, user_id: &str) -> Result<(), String> {
    let room = room_by_id(room_id).await?;
    let uid = matrix_sdk::ruma::UserId::parse(user_id).map_err(|e| format!("bad user id: {e}"))?;
    room.invite_user_by_id(&uid).await.map_err(|e| e.to_string())?;
    Ok(())
}

async fn do_moderate(
    room_id: &str,
    user_id: &str,
    reason: Option<&str>,
    action: ModAction,
) -> Result<(), String> {
    let room = room_by_id(room_id).await?;
    let uid = matrix_sdk::ruma::UserId::parse(user_id).map_err(|e| format!("bad user id: {e}"))?;
    if matches!(action, ModAction::Kick) {
        // a stale entry (kicked / left already, guest-leave 12.1): the server would only answer about permissions
        // (left_words.rs: they left, or someone else removed them, while the confirmation was open)
        if let Ok(Some(m)) = room.get_member_no_sync(&uid).await {
            let who = m.display_name().map(str::to_owned).unwrap_or_else(|| user_id.to_owned());
            if let Some(gone) = crate::left_words::gone_text(crate::left_words::LeftScope::Room, &who, Some(m.membership().as_str()), false) {
                return Err(gone);
            }
        }
    }
    match action {
        ModAction::Kick => room.kick_user(&uid, reason).await.map_err(|e| e.to_string())?,
        ModAction::Ban => room.ban_user(&uid, reason).await.map_err(|e| e.to_string())?,
        ModAction::Unban => room.unban_user(&uid, reason).await.map_err(|e| e.to_string())?,
    };
    Ok(())
}

async fn do_set_power(room_id: &str, user_id: &str, power: i32) -> Result<(), String> {
    let room = room_by_id(room_id).await?;
    let uid = matrix_sdk::ruma::UserId::parse(user_id).map_err(|e| format!("bad user id: {e}"))?;
    // a person-chosen moderator action (allowed in any room); under the room's power-level lock, from the same
    // up-to-date state and through the same echo wait as ensure_writable / the roles reconciler (the web's setMemberPower)
    let lock = crate::appstate::power_level_lock(room_id);
    let _held = lock.lock().await;
    let mut pl = crate::appstate::power_levels_for_write(&room)
        .await
        .ok_or_else(|| "the room's power levels are still updating — try again in a moment".to_string())?;
    if !pl.get("users").map(serde_json::Value::is_object).unwrap_or(false) {
        pl["users"] = serde_json::json!({});
    }
    if let Some(users) = pl.get_mut("users").and_then(serde_json::Value::as_object_mut) {
        users.insert(uid.to_string(), serde_json::Value::from(power));
    }
    crate::appstate::send_power_levels(&room, pl).await.map_err(|e| e.to_string())?;
    Ok(())
}

/// Our own power level + which moderation actions the room's power-level event permits
/// us to perform. Pushed into the controller's qproperties to gate UI affordances.
#[derive(Default)]
struct PermissionFlags {
    can_pin: bool,
    /// widgets W3: may send m.room.message (Dice: roll to the room)
    can_send_message: bool,
    /// widgets W3: may send the poll start event (Polls: New poll)
    can_send_poll: bool,
    my_power: i32,
    can_change_name: bool,
    can_change_topic: bool,
    can_invite: bool,
    can_kick: bool,
    can_ban: bool,
    can_change_power: bool,
}
impl PermissionFlags {
    fn apply(&self, mut o: Pin<&mut qobject::RoomController>) {
        o.as_mut().set_my_power(self.my_power);
        o.as_mut().set_can_change_name(self.can_change_name);
        o.as_mut().set_can_change_topic(self.can_change_topic);
        o.as_mut().set_can_invite(self.can_invite);
        o.as_mut().set_can_kick(self.can_kick);
        o.as_mut().set_can_ban(self.can_ban);
        o.as_mut().set_can_change_power(self.can_change_power);
        o.as_mut().set_can_pin(self.can_pin);
        o.as_mut().set_can_send_message(self.can_send_message);
        o.as_mut().set_can_send_poll(self.can_send_poll);
    }
}

/// Compute our permission flags from the room's power-level state. All checks are
/// against the local store (no network). Defaults to all-false on any error so the UI
/// fails closed (no affordance) rather than showing an action that will 403.
async fn load_permissions(room: &Room) -> PermissionFlags {
    use matrix_sdk::ruma::events::room::power_levels::UserPowerLevel;
    use matrix_sdk::ruma::events::StateEventType;
    let me = room.own_user_id().to_owned();
    let Ok(pl) = room.power_levels().await else {
        return PermissionFlags::default();
    };
    let my_power = match pl.for_user(&me) {
        UserPowerLevel::Infinite => 100,
        UserPowerLevel::Int(i) => i64::from(i).clamp(i32::MIN as i64, i32::MAX as i64) as i32,
        _ => 0,
    };
    PermissionFlags {
        my_power,
        can_change_name: pl.user_can_send_state(&me, StateEventType::RoomName),
        can_change_topic: pl.user_can_send_state(&me, StateEventType::RoomTopic),
        can_invite: pl.user_can_invite(&me),
        can_kick: pl.user_can_kick(&me),
        can_ban: pl.user_can_ban(&me),
        can_change_power: pl.user_can_send_state(&me, StateEventType::RoomPowerLevels),
        can_pin: pl.user_can_send_state(&me, StateEventType::RoomPinnedEvents),
        can_send_message: pl.user_can_send_message(&me, matrix_sdk::ruma::events::MessageLikeEventType::RoomMessage),
        can_send_poll: pl.user_can_send_message(&me, matrix_sdk::ruma::events::MessageLikeEventType::from(tinhat_widgets::polls::START_TYPE)),
    }
}

/// `load_permissions` for the refresh() path, which only has a room id in scope.
async fn load_permissions_by_id(room_id: &str) -> PermissionFlags {
    match room_by_id(room_id).await {
        Ok(room) => load_permissions(&room).await,
        Err(_) => PermissionFlags::default(),
    }
}

// ---- 9.3: local echo -----------------------------------------------------------------
// What you send is on screen before the network is. An Echo lives in a process-wide store
// keyed by room (so a room switch and back still shows a pending or failed line), renders
// as a timeline row with a "~txn" id, and is dropped the moment the server's own copy is
// in the window.

#[derive(Clone, Debug)]
pub(crate) enum EchoKind {
    Text,
    Emote,
    /// 10.6 (A5.2): the @-mentions picked while replying, `(name, userId)`.
    Reply(Vec<(String, String)>),
    Mentions(Vec<(String, String)>),
    /// A local file path (upload + send).
    Attachment(String),
    /// widgets W3: a dice roll — the shared message content (m.notice + HTML + `dev.tinhat.dice`), sent as is.
    Dice(serde_json::Value),
    /// widgets W3: a new poll — the shared start content (Element's shape), sent as the poll start event.
    Poll(serde_json::Value),
    /// voice notes (docs/VOICE-NOTES.md): a finished recording (upload + voice_content + send_raw, same txn on retry).
    Voice(VoiceSend),
}

/// voice notes: what the echo engine needs to send a finished recording.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct VoiceSend {
    pub path: String,
    pub duration_ms: u64,
    pub waveform: Vec<u32>,
    pub size: u64,
}

/// voice notes (§1 "retry reuses the finished upload"): the finished uploads per txn are kept by the voice-note
/// machine (voicenote_ctl), under the drafts' own lock (review r2 #4).
pub(crate) fn voice_upload_get(txn: &str) -> Option<(tinhat_widgets::voicenote::VoiceMedia, u64)> {
    crate::voicenote_ctl::upload_get(txn)
}
pub(crate) fn voice_upload_forget(txn: &str) {
    crate::voicenote_ctl::upload_forget(txn)
}
/// review r1 #1: may a cached upload be sent in a room whose encryption is `encrypted`? A plaintext (`url`) upload never
/// goes into an encrypted room; an encrypted one is fine anywhere (rooms cannot turn encryption off).
/// Security review round 4 (VN-1 parity, VN-2): a voice note's upload is encrypted when the read at send time OR a
/// synchronous re-read right before the upload starts says so (the file read and the limit lookup awaited between
/// them: this closes that window; sync runs on other tasks, so a change landing during the upload itself or between the
/// post-upload check and send_raw is only narrowed, not excluded — the post-upload check re-uploads encrypted when it
/// sees it); a state the SDK can't tell (Unknown) uploads nothing. matrix-sdk's encryption state is sticky (a known m.room.encryption is never forgotten by sync; the server
/// is asked only while it is unknown) and send_raw asks the same state, so the upload and the event can't disagree the
/// way the web's room-state-only check could (the web asks js-sdk's crypto store too: core/matrix/uploadCrypto.ts).
pub(crate) fn voice_upload_encrypt(at_send: &matrix_sdk::EncryptionState, at_upload: &matrix_sdk::EncryptionState) -> Result<bool, String> {
    if at_send.is_unknown() || at_upload.is_unknown() {
        return Err("voice: the room's encryption can't be checked, so nothing was uploaded".into());
    }
    Ok(at_send.is_encrypted() || at_upload.is_encrypted())
}
pub(crate) fn voice_upload_fits(media: &tinhat_widgets::voicenote::VoiceMedia, encrypted: bool) -> bool {
    !encrypted || matches!(media, tinhat_widgets::voicenote::VoiceMedia::File(_))
}
/// §6: refuse before uploading only when the server's limit is KNOWN (a missing / unreadable / zero limit = unknown).
/// A voice note's or an attachment's size against the server's m.upload.size (None = unknown: never refused). Shared
/// vectors src/testvectors/upload-limit.json (the web's overUploadLimit).
pub(crate) fn voice_over_limit(size: u64, limit: Option<u64>) -> bool {
    limit.is_some_and(|l| l > 0 && size > l)
}

#[cfg(test)]
mod react_ids_tests {
    /// QOL A07.2 review: once my reaction on a chip is taken back (or re-added optimistically), the chip's list of my
    /// server ids is gone with it — a later take-back must not redact the old ids instead of the new reaction.
    #[test]
    fn optimistic_changes_drop_my_ids() {
        let json = r#"[{"id":"$m","reactions":[{"key":"❤","count":2,"mine":true,"reactionId":"$a","reactionIds":["$a","$b"]}]}]"#;
        let removed = super::apply_optimistic_react(json, "$m", "❤️", false).expect("removed");
        let v: serde_json::Value = serde_json::from_str(&removed).unwrap();
        assert_eq!(v[0]["reactions"][0]["reactionIds"], serde_json::json!([]));
        let readded = super::apply_optimistic_react(&removed, "$m", "❤", true).expect("re-added");
        let v: serde_json::Value = serde_json::from_str(&readded).unwrap();
        assert_eq!(v[0]["reactions"][0]["reactionId"], super::OPTIMISTIC_RID);
        assert_eq!(v[0]["reactions"][0]["reactionIds"], serde_json::json!([]));
    }
}

#[cfg(test)]
mod upload_limit_tests {
    /// QOL A08.2: the shared upload-limit vectors.
    #[test]
    fn upload_limit_vectors() {
        let path = format!("{}/src/testvectors/upload-limit.json", env!("CARGO_MANIFEST_DIR"));
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let cases = v["cases"].as_array().unwrap();
        assert!(cases.len() >= 7);
        for c in cases {
            let got = super::voice_over_limit(c["size"].as_u64().unwrap(), c["limit"].as_u64());
            assert_eq!(got, c["over"].as_bool().unwrap(), "{}", c["name"]);
        }
    }
    /// The attachment send checks the limit before it reads the file (failing-first: the read came first).
    #[test]
    fn attachment_checks_the_limit_before_reading() {
        let src = include_str!("room.rs");
        let arm = src.split("EchoKind::Attachment(path) => {\n            use matrix_sdk::ruma::events::room::MediaSource;").nth(1).unwrap();
        let check = arm.find("voice_over_limit(size, limit)").expect("the attachment path checks the upload limit");
        let read = arm.find("tokio::fs::read(&pb)").unwrap();
        assert!(check < read, "the limit is checked before the file is read");
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Echo {
    pub txn: String,
    pub kind: EchoKind,
    pub body: String,
    pub msgtype: String,
    pub reply_to: String,
    /// 10.6 (A5.2): the parent's sender, from the loaded timeline when the reply was made — so a
    /// failed lookup at send time can never drop the mention of the person replied to.
    pub reply_sender: String,
    pub media: String,
    pub mime: String,
    pub size: u64,
    /// 9.3.2: pixel size of an image attachment (0 = unknown) — the echo row's `w`/`h` and
    /// the m.image `info.w/h` sent with it, so every timeline reserves the height up front.
    pub w: u32,
    pub h: u32,
    /// voice notes: the note's duration (the echo row's `durationMs`); 0 for everything else
    pub duration_ms: u64,
    pub ts: u64,
    /// Set once the server accepted it (the send's response).
    pub event_id: Option<String>,
    /// Set when the send failed (a friendly message; retry keeps the row).
    pub failed: Option<String>,
    /// threads (docs/THREADS.md): the thread it is sent into ("" = the main timeline) and that thread's newest event
    /// when it was made (the spec's fallback reply target); a reply's parent's thread (A07.3: the reply stays in it)
    pub thread: String,
    pub thread_latest: String,
    pub reply_thread: String,
    /// L1 D08.2: the failure's class (fail_class) — a "network" one sends itself again when the connection is back.
    pub fail_cls: &'static str,
    /// L1 D08.2: automatic resends so far (resendable: at most MAX_AUTO_TRIES; a Retry starts over).
    pub auto_tries: u8,
}

impl Echo {
    pub fn new(kind: EchoKind, body: &str) -> Self {
        let msgtype = match kind {
            EchoKind::Emote => "m.emote",
            EchoKind::Dice(_) => "m.notice",
            EchoKind::Poll(_) => POLL_MSGTYPE,
            _ => "m.text",
        };
        Self {
            txn: matrix_sdk::ruma::TransactionId::new().to_string(),
            kind,
            body: body.to_string(),
            msgtype: msgtype.to_string(),
            reply_to: String::new(),
            reply_sender: String::new(),
            media: String::new(),
            mime: String::new(),
            size: 0,
            w: 0,
            h: 0,
            duration_ms: 0,
            ts: now_ms(),
            event_id: None,
            failed: None,
            thread: String::new(),
            thread_latest: String::new(),
            reply_thread: String::new(),
            fail_cls: "",
            auto_tries: 0,
        }
    }
}

/// threads: the relation a send gets. A reply: tinhat_widgets::threads::reply_relation (its parent's thread, else the
/// panel's thread: A07.3). Anything else from a thread's panel: the thread relation (the spec's fallback reply to the
/// thread's newest event). None = leave the content as built (a main-timeline send).
pub(crate) fn echo_relation(e: &Echo) -> Option<serde_json::Value> {
    use tinhat_widgets::threads::{reply_relation, thread_relation};
    if !e.reply_to.is_empty() {
        let parent_thread = if e.reply_thread.is_empty() { e.thread.as_str() } else { e.reply_thread.as_str() };
        return Some(reply_relation(&e.reply_to, parent_thread));
    }
    if e.thread.is_empty() {
        return None;
    }
    Some(thread_relation(&e.thread, &e.thread_latest))
}

/// threads: send `content` (typed) as built, or — for a thread send — as JSON with its relation added.
async fn send_maybe_threaded(room: &Room, e: &Echo, content: RoomMessageEventContent, txn: &matrix_sdk::ruma::TransactionId) -> Result<String, String> {
    match echo_relation(e) {
        None => {
            let r = room.send(content).with_transaction_id(txn.to_owned()).await.map_err(|x| x.to_string())?;
            Ok(r.response.event_id.to_string())
        }
        Some(rel) => {
            let mut v = serde_json::to_value(content).map_err(|x| x.to_string())?;
            v["m.relates_to"] = rel;
            let r = room.send_raw("m.room.message", v).with_transaction_id(txn).await.map_err(|x| x.to_string())?;
            Ok(r.response.event_id.to_string())
        }
    }
}

/// widgets W3: a timeline's dice rows (server rows + my echoes; not deleted ones) for the Dice widget's room history —
/// the verified field back (the shared history re-reads it; the order is the history's business).
pub(crate) fn dice_rows(msgs: &[serde_json::Value]) -> serde_json::Value {
    let rows: Vec<serde_json::Value> = msgs
        .iter()
        .filter(|m| m.get("dice").is_some_and(|d| d.is_object()) && m.get("redacted") != Some(&serde_json::Value::Bool(true)) && m.get("sample").is_none())
        .map(|m| {
            let d = &m["dice"];
            serde_json::json!({
                "id": m.get("id").cloned().unwrap_or_default(), "sender": m.get("sender").cloned().unwrap_or_default(),
                "senderName": m.get("senderName").cloned().unwrap_or_default(), "avatar": m.get("avatar").cloned().unwrap_or_default(),
                "ts": m.get("ts").cloned().unwrap_or_default(),
                "field": { "v": 1, "expr": d["expr"], "rolls": d["rolls"], "kept": d["kept"], "total": d["total"] },
                "pending": m.get("pending") == Some(&serde_json::Value::Bool(true)), "failed": m.get("failed") == Some(&serde_json::Value::Bool(true)),
            })
        })
        .collect();
    serde_json::Value::Array(rows)
}

/// widgets W3: the msgtype of a poll start row (not a real msgtype: poll starts are events of their own; it keeps the row
/// out of the text features — edit, embeds, link previews).
pub(crate) const POLL_MSGTYPE: &str = "org.matrix.msc3381.poll";

/// widgets W3: a fresh poll answer id — 12 characters [a-z0-9] from the OS CSPRNG (unbiased: bytes ≥ 252 are drawn again).
fn poll_answer_id() -> String {
    use rand::RngCore;
    const A: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut out = String::new();
    let mut buf = [0u8; 32];
    while out.len() < 12 {
        rand::rngs::OsRng.fill_bytes(&mut buf);
        for b in buf {
            if b < 252 && out.len() < 12 {
                out.push(A[(b % 36) as usize] as char);
            }
        }
    }
    out
}

/// widgets W3: every poll row's card (rows with `pollType` + `pollContent`; my unsent poll keeps its pending card) from the
/// window's responses and ends.
pub(crate) fn attach_poll_cards(msgs: &mut [serde_json::Value], rels: &[serde_json::Value], enders: &[String], me: &str, can_redact: bool) {
    for m in msgs.iter_mut() {
        if m.get("redacted") == Some(&serde_json::Value::Bool(true)) || m.get("echo") == Some(&serde_json::Value::Bool(true)) {
            continue;
        }
        let (Some(ty), Some(content)) = (m.get("pollType").and_then(|t| t.as_str()).map(String::from), m.get("pollContent").cloned()) else { continue };
        let id = m.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();
        let creator = m.get("sender").and_then(|s| s.as_str()).unwrap_or("").to_string();
        if let Some((card, _, _)) = crate::polls_ctl::card_of(&ty, &content, &id, &creator, rels, enders, me, can_redact, false) {
            m["poll"] = card;
        }
    }
}

/// widgets W3: the open polls of the rows (server rows + my pending one; not samples, not failed) for the Polls widget —
/// {id, ts, question, line}, newest first.
pub(crate) fn poll_rows(msgs: &[serde_json::Value]) -> serde_json::Value {
    let mut rows: Vec<(f64, serde_json::Value)> = msgs
        .iter()
        .filter(|m| m.get("sample").is_none() && m.get("failed") != Some(&serde_json::Value::Bool(true)) && m.get("redacted") != Some(&serde_json::Value::Bool(true)))
        .filter_map(|m| {
            let c = m.get("poll")?;
            if !c.is_object() || c.get("ended") == Some(&serde_json::Value::Bool(true)) {
                return None;
            }
            let ts = m.get("ts").and_then(|t| t.as_f64()).unwrap_or(0.0);
            Some((ts, serde_json::json!({ "id": m.get("id").cloned().unwrap_or_default(), "ts": ts, "question": c["question"], "line": c["line"] })))
        })
        .collect();
    rows.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    serde_json::Value::Array(rows.into_iter().map(|r| r.1).collect())
}

fn poll_rows_json(msgs_json: &str) -> String {
    let v: Vec<serde_json::Value> = serde_json::from_str::<serde_json::Value>(msgs_json).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
    poll_rows(&v).to_string()
}

/// widgets W3: the gates' sample conversation rows — `TINHAT_CHAT_SAMPLES=dice` appends the shared sample dice messages
/// (tinhat_widgets::dice::DICE_CHAT_SAMPLES; the web's `?chatsamples=dice` shows the same rows) to the open conversation.
/// Only that env var (the gates / reviews) ever sets it: normal use never sees them.
fn chat_sample_rows(my_id: &str, my_name: &str) -> Vec<serde_json::Value> {
    let kinds = std::env::var("TINHAT_CHAT_SAMPLES").unwrap_or_default();
    let want = |k: &str| kinds.split(',').any(|x| x.trim() == k);
    if !want("dice") && !want("polls") {
        return Vec::new();
    }
    let now = now_ms() as i64;
    let off = local_offset_ms(now);
    let day = 86_400_000i64;
    let midnight = (now + off).div_euclid(day) * day - off;
    let mut out = Vec::new();
    // widgets W3: the sample polls (tinhat_widgets::polls::POLL_CHAT_SAMPLES; the web's ?chatsamples=polls): each start a
    // row with its card, tallied from its own sample responses and end
    if want("polls") {
        for s in tinhat_widgets::polls::POLL_CHAT_SAMPLES {
            let ev = tinhat_widgets::polls::sample_events(s, my_id, midnight as f64);
            let Some(first) = ev.first() else { continue };
            let ty = first["type"].as_str().unwrap_or("");
            let sender = first["sender"].as_str().unwrap_or("").to_string();
            let Some((card, poll, _)) = crate::polls_ctl::card_of(ty, &first["content"], s.id, &sender, &ev, &[], my_id, false, false) else { continue };
            let name = if s.mine { my_name.to_string() } else { sender.trim_start_matches('@').split(':').next().unwrap_or("").to_string() };
            out.push(serde_json::json!({
                "sender": sender, "senderName": name, "body": format!("Poll: {}", poll.question), "ts": first["ts"], "id": s.id,
                "avatar": "", "mentionsMe": false, "pinned": false, "serverTxn": "",
                "msgtype": POLL_MSGTYPE, "formatted": "", "media": "",
                "mxc": "", "mimetype": "", "w": 0, "h": 0, "durationMs": 0, "size": 0,
                "replyTo": "", "redacted": false, "embeds": [], "onlyEmbed": false,
                "calContainer": "", "calEvent": "", "themeShare": serde_json::Value::Null, "pinSnippet": serde_json::Value::Null,
                "dice": serde_json::Value::Null, "poll": card,
                "sample": true,
            }));
        }
    }
    if !want("dice") {
        return out;
    }
    for s in tinhat_widgets::dice::DICE_CHAT_SAMPLES {
        let Some(content) = tinhat_widgets::dice::sample_content(s) else { continue };
        let Some(r) = tinhat_widgets::dice::read_dice(&content) else { continue };
        let (sender, name) = if s.mine {
            (my_id.to_string(), my_name.to_string())
        } else {
            (s.sender.to_string(), s.sender.trim_start_matches('@').split(':').next().unwrap_or("").to_string())
        };
        let ts = midnight + (s.at.0 as i64 * 60 + s.at.1 as i64) * 60_000;
        out.push(serde_json::json!({
            "sender": sender, "senderName": name, "body": content["body"], "ts": ts, "id": s.id,
            "avatar": "", "mentionsMe": false, "pinned": false, "serverTxn": "",
            "msgtype": "m.notice", "formatted": content["formatted_body"], "media": "",
            "mxc": "", "mimetype": "", "w": 0, "h": 0, "durationMs": 0, "size": 0,
            "replyTo": "", "redacted": false, "embeds": [], "onlyEmbed": false,
            "calContainer": "", "calEvent": "", "themeShare": serde_json::Value::Null, "pinSnippet": serde_json::Value::Null,
            "dice": crate::dice_ctl::card_json(&r),
            // a gate sample: the chat only (the web's samples never reach the widget's history either)
            "sample": true,
        }));
    }
    out
}

/// This machine's UTC offset (ms) at `now_ms` (the samples' local times); 0 where it can't be read.
pub(crate) fn local_offset_ms(now_ms: i64) -> i64 {
    #[cfg(unix)]
    {
        let t = (now_ms / 1000) as libc::time_t;
        // SAFETY: localtime_r only writes the tm we own
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
            return 0;
        }
        tm.tm_gmtoff as i64 * 1000
    }
    #[cfg(not(unix))]
    {
        let _ = now_ms;
        0
    }
}

fn dice_rows_json(msgs_json: &str) -> String {
    let v: Vec<serde_json::Value> = serde_json::from_str::<serde_json::Value>(msgs_json).ok().and_then(|v| v.as_array().cloned()).unwrap_or_default();
    dice_rows(&v).to_string()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn echo_store() -> &'static std::sync::Mutex<std::collections::HashMap<String, Vec<Echo>>> {
    static S: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, Vec<Echo>>>> =
        std::sync::OnceLock::new();
    S.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn echo_store_push(room_id: &str, echo: Echo) {
    let mut s = echo_store().lock().unwrap();
    let v = s.entry(room_id.to_string()).or_default();
    v.retain(|e| e.txn != echo.txn);
    v.push(echo);
}

fn echo_store_take(room_id: &str, txn: &str) -> Option<Echo> {
    let mut s = echo_store().lock().unwrap();
    let v = s.get_mut(room_id)?;
    let i = v.iter().position(|e| e.txn == txn)?;
    Some(v.remove(i))
}

fn echo_store_sent(room_id: &str, txn: &str, event_id: &str) {
    if let Some(v) = echo_store().lock().unwrap().get_mut(room_id) {
        if let Some(e) = v.iter_mut().find(|e| e.txn == txn) {
            e.event_id = Some(event_id.to_string());
            e.failed = None;
        }
    }
}

fn echo_store_failed(room_id: &str, txn: &str, err: &str) {
    if let Some(v) = echo_store().lock().unwrap().get_mut(room_id) {
        if let Some(e) = v.iter_mut().find(|e| e.txn == txn) {
            e.failed = Some(friendly_send_error(err));
            e.fail_cls = fail_class(err);
        }
    }
}

/// L1 D08.2: does a failed message send itself again when the connection is back? (testvectors/resend-after-reconnect,
/// Tin Hat Web's lib/resend.ts.) Only a failure for lack of connection; never a voice note (its draft owns the retry).
pub(crate) fn resendable(cls: &str, voice: bool, tries: u8) -> bool {
    cls == "network" && !voice && tries < MAX_AUTO_TRIES
}

/// L1 D08.2: how many times a message resends by itself (testvectors/resend-after-reconnect maxAutoTries).
pub(crate) const MAX_AUTO_TRIES: u8 = 5;

/// L1 D08.2: a good sync (the first after an outage, or any later one while such a message waits): every message that
/// failed for lack of connection is sent again (at most MAX_AUTO_TRIES times by itself), per room oldest first, with its own transaction id (the server drops a duplicate). Each one is marked
/// pending at once and its room's open view repaints ("echo:<room>" on keys_channel) as it starts and as it ends.
pub(crate) fn resend_after_reconnect() {
    // 4-3: the batch belongs to THIS session: its client and generation are taken now; a sign-out (or a switch to another
    // account) bumps the generation and the batch stops before its next send (resend_batch)
    let gen = crate::session::generation();
    let Some(client) = crate::session::client() else { return };
    let mut todo: Vec<(String, Vec<Echo>)> = Vec::new();
    {
        let mut s = echo_store().lock().unwrap();
        for (room_id, v) in s.iter_mut() {
            let mut list = Vec::new();
            for e in v.iter_mut() {
                if e.failed.is_some() && e.event_id.is_none() && resendable(e.fail_cls, matches!(e.kind, EchoKind::Voice(_)), e.auto_tries) {
                    e.failed = None;
                    e.fail_cls = "";
                    e.auto_tries += 1;
                    list.push(e.clone());
                }
            }
            if !list.is_empty() {
                todo.push((room_id.clone(), list));
            }
        }
    }
    for (room_id, list) in todo {
        eprintln!("[room] connection back: sending {} failed message(s) again in {room_id}", list.len());
        let _ = keys_channel().send(format!("echo:{room_id}"));
        let client = client.clone();
        crate::bridge::runtime().spawn(async move {
            let rid = room_id.clone();
            resend_batch(gen, crate::session::generation, &room_id, list, move |e| {
                let (c, rid) = (client.clone(), rid.clone());
                async move { perform_echo(&c, &rid, &e).await }
            })
            .await;
        });
    }
}

/// 4-3: one room's resend batch, oldest first. Before each send, and again when its answer comes, the session must
/// still be the one that queued it (`now_gen() == gen`); otherwise the rest is dropped (sign-out already cleared the
/// echo store, forget_session) and nothing is recorded. `send` sends through the batch's own client. Returns how many
/// were sent or failed while the session lasted.
async fn resend_batch<S, F>(gen: u64, now_gen: impl Fn() -> u64, room_id: &str, list: Vec<Echo>, mut send: S) -> usize
where
    S: FnMut(Echo) -> F,
    F: std::future::Future<Output = Result<String, String>>,
{
    let total = list.len();
    let mut done = 0;
    for e in list {
        if now_gen() != gen {
            eprintln!("[room] the session ended: {} queued resend(s) dropped in {room_id}", total - done);
            return done;
        }
        let res = send(e.clone()).await;
        if now_gen() != gen {
            eprintln!("[room] the session ended during a resend: {} queued resend(s) dropped in {room_id}", total - done - 1);
            return done;
        }
        match res {
            Ok(eid) => echo_store_sent(room_id, &e.txn, &eid),
            Err(err) => {
                eprintln!("[room] resend failed ({}): {err}", e.txn);
                echo_store_failed(room_id, &e.txn, &err);
            }
        }
        done += 1;
        let _ = keys_channel().send(format!("echo:{room_id}"));
    }
    done
}

/// The echoes of `room_id` the given server rows do NOT yet contain, oldest first. Echoes
/// the rows DO contain are removed from the store here (delivered).
/// threads: only the echoes sent into `view` ("" = the main timeline, else that thread) are looked at.
fn echo_store_undelivered(room_id: &str, msgs: &[serde_json::Value], view: &str) -> Vec<Echo> {
    let my_id = crate::session::client()
        .and_then(|c| c.user_id().map(|u| u.to_string()))
        .unwrap_or_default();
    let mut s = echo_store().lock().unwrap();
    let Some(v) = s.get_mut(room_id) else { return Vec::new() };
    let delivered = |e: &Echo| -> bool {
        msgs.iter().any(|m| {
            let id = m.get("id").and_then(|i| i.as_str()).unwrap_or("");
            if let Some(eid) = &e.event_id {
                if id == eid {
                    return true;
                }
            }
            if m.get("serverTxn").and_then(|t| t.as_str()) == Some(e.txn.as_str()) {
                return true;
            }
            // Uploads (no event id captured until the upload finished) and anything else
            // the server showed us under a different handle: mine, same type + text, sent
            // after this echo was.
            m.get("sender").and_then(|x| x.as_str()) == Some(my_id.as_str())
                && m.get("msgtype").and_then(|x| x.as_str()) == Some(e.msgtype.as_str())
                && m.get("body").and_then(|x| x.as_str()) == Some(e.body.as_str())
                && m.get("ts").and_then(|x| x.as_u64()).unwrap_or(0) + 120_000 >= e.ts
        })
    };
    let mut out = Vec::new();
    v.retain(|e| {
        if e.thread != view {
            return true;
        }
        if delivered(e) {
            false
        } else {
            out.push(e.clone());
            true
        }
    });
    out.sort_by_key(|e| e.ts);
    out
}

/// The timeline row for an echo. `cur_json` = the rows it will sit among (a reply quote
/// is resolved from them).
fn echo_row(e: &Echo, my_id: &str, my_name: &str, my_avatar: &str, cur_json: &str) -> serde_json::Value {
    let (mut reply_sender, mut reply_body) = (String::new(), String::new());
    if !e.reply_to.is_empty() {
        if let Ok(serde_json::Value::Array(rows)) = serde_json::from_str::<serde_json::Value>(cur_json) {
            if let Some(p) = rows.iter().find(|m| m.get("id").and_then(|i| i.as_str()) == Some(e.reply_to.as_str())) {
                reply_sender = p.get("senderName").and_then(|s| s.as_str()).unwrap_or("").to_string();
                reply_body = p.get("body").and_then(|s| s.as_str()).unwrap_or("").to_string();
            }
        }
    }
    let formatted = match &e.kind {
        EchoKind::Mentions(m) => render_mention_html(&e.body, m),
        EchoKind::Reply(m) if !m.is_empty() => render_mention_html(&e.body, m),
        _ => String::new(),
    };
    serde_json::json!({
        "id": format!("~{}", e.txn),
        "txn": e.txn,
        "echo": true,
        "threadId": e.thread,
        "pending": e.event_id.is_none() && e.failed.is_none(),
        "failed": e.failed.is_some(),
        "error": e.failed.clone().unwrap_or_default(),
        "sentId": e.event_id.clone().unwrap_or_default(),
        "sender": my_id, "senderName": my_name, "avatar": my_avatar,
        "body": e.body, "ts": e.ts, "msgtype": e.msgtype, "formatted": formatted,
        "media": e.media, "mxc": "", "mimetype": e.mime, "w": e.w, "h": e.h,
        "durationMs": e.duration_ms, "size": e.size, "voice": matches!(e.kind, EchoKind::Voice(_)),
        "replyTo": e.reply_to, "replyToSender": reply_sender, "replyToBody": reply_body,
        "redacted": false, "edited": false, "mentionsMe": false, "pinned": false,
        "embeds": crate::embeds::extract_embeds(&e.body),
        "onlyEmbed": crate::embeds::body_is_only_embed(&e.body),
        "calContainer": "", "calEvent": "", "themeShare": serde_json::Value::Null,
        // widgets W3: a dice roll's echo shows its card at once
        "dice": match &e.kind {
            EchoKind::Dice(c) => tinhat_widgets::dice::read_dice(c).map_or(serde_json::Value::Null, |r| crate::dice_ctl::card_json(&r)),
            _ => serde_json::Value::Null,
        },
        // widgets W3: a new poll's echo shows its card at once (pending: nobody can vote yet)
        "poll": match &e.kind {
            EchoKind::Poll(c) => crate::polls_ctl::card_of(tinhat_widgets::polls::START_TYPE, c, "", my_id, &[], &[], my_id, false, true).map_or(serde_json::Value::Null, |x| x.0),
            _ => serde_json::Value::Null,
        },
        "pollType": if matches!(e.kind, EchoKind::Poll(_)) { tinhat_widgets::polls::START_TYPE } else { "" },
    })
}

/// The real send for an echo; returns the server's event id.
async fn perform_echo(client: &matrix_sdk::Client, room_id: &str, e: &Echo) -> Result<String, String> {
    use matrix_sdk::ruma::OwnedTransactionId;
    // 4-3: the room comes from the client the send belongs to (a queued send never resolves through the next session's)
    let room = client.get_room(&RoomId::parse(room_id).map_err(|x| format!("bad room id: {x}"))?).ok_or_else(|| "Room not found".to_string())?;
    let txn: OwnedTransactionId = e.txn.as_str().into();
    match &e.kind {
        // threads: a thread's panel sends with the thread relation (send_maybe_threaded); the main timeline as before
        EchoKind::Text => send_maybe_threaded(&room, e, RoomMessageEventContent::text_markdown(&e.body), &txn).await,
        EchoKind::Emote => send_maybe_threaded(&room, e, RoomMessageEventContent::emote_markdown(&e.body), &txn).await,
        EchoKind::Reply(picked) => {
            // 10.6 (REACH A5.2): a reply notifies the person replied to — `m.mentions` carries the
            // parent's sender (never yourself) plus the mentions picked in the composer, whose
            // links stay in the body. Once `m.mentions` is present Synapse stops name-matching,
            // so the picks MUST be in it too.
            use matrix_sdk::ruma::{events::Mentions, OwnedUserId, UserId};
            let me = room.own_user_id().to_string();
            // the timeline's own record first; the event lookup only when the parent was not loaded
            let parent_sender = if e.reply_sender.is_empty() {
                reply_parent_sender(&room, &e.reply_to).await
            } else {
                Some(e.reply_sender.clone())
            };
            let picks: Vec<String> = picked.iter().map(|(_, u)| u.clone()).collect();
            let ids = reply_mention_ids(parent_sender.as_deref(), &me, &picks);
            let mut base = if picked.is_empty() {
                RoomMessageEventContent::text_markdown(&e.body)
            } else {
                RoomMessageEventContent::text_html(&e.body, render_mention_html(&e.body, picked))
            };
            let uids: Vec<OwnedUserId> = ids.iter().filter_map(|u| UserId::parse(u).ok()).collect();
            if !uids.is_empty() {
                base = base.add_mentions(Mentions::with_user_ids(uids));
            }
            let mut content = serde_json::to_value(base).map_err(|x| x.to_string())?;
            // A07.3 (threads): a reply to a message in a thread stays in that thread (echo_relation → reply_relation)
            content["m.relates_to"] = echo_relation(e).unwrap_or_else(|| serde_json::json!({ "m.in_reply_to": { "event_id": e.reply_to } }));
            let r = room
                .send_raw("m.room.message", content)
                .with_transaction_id(&txn)
                .await
                .map_err(|x| x.to_string())?;
            Ok(r.response.event_id.to_string())
        }
        EchoKind::Mentions(mentions) => {
            use matrix_sdk::ruma::{events::Mentions, OwnedUserId, UserId};
            let html = render_mention_html(&e.body, mentions);
            let ids: Vec<OwnedUserId> = mentions.iter().filter_map(|(_, u)| UserId::parse(u).ok()).collect();
            let mut content = RoomMessageEventContent::text_html(&e.body, html);
            if !ids.is_empty() {
                content = content.add_mentions(Mentions::with_user_ids(ids));
            }
            send_maybe_threaded(&room, e, content, &txn).await
        }
        EchoKind::Dice(content) => {
            // threads (review #11): a roll from a thread's panel goes into the thread
            let mut content = content.clone();
            if let Some(rel) = echo_relation(e) {
                content["m.relates_to"] = rel;
            }
            let r = room
                .send_raw("m.room.message", content)
                .with_transaction_id(&txn)
                .await
                .map_err(|x| x.to_string())?;
            Ok(r.response.event_id.to_string())
        }
        EchoKind::Poll(content) => {
            // threads (review #11): a poll from a thread's panel starts in the thread
            let mut content = content.clone();
            if let Some(rel) = echo_relation(e) {
                content["m.relates_to"] = rel;
            }
            let r = room
                .send_raw(tinhat_widgets::polls::START_TYPE, content)
                .with_transaction_id(&txn)
                .await
                .map_err(|x| x.to_string())?;
            Ok(r.response.event_id.to_string())
        }
        EchoKind::Attachment(path) => {
            use matrix_sdk::ruma::events::room::MediaSource;
            // refused before any upload (the SDK's attachment path did the same): nothing is left behind on the server
            if room.state() != matrix_sdk::RoomState::Joined {
                return Err("not joined".into());
            }
            let pb = std::path::PathBuf::from(path);
            let client = room.client();
            // QOL A08.2: the server's m.upload.size (cached by the SDK; unknown = no refusal), checked BEFORE the file is
            // read: a file over it is refused with "too large" and never read into memory or uploaded
            let size = tokio::fs::metadata(&pb).await.map(|m| m.len()).unwrap_or(e.size);
            let limit = client.load_or_fetch_max_upload_size().await.ok().map(u64::from);
            if voice_over_limit(size, limit) {
                return Err(format!("M_TOO_LARGE ({size} bytes over the server's {} byte limit)", limit.unwrap_or(0)));
            }
            // tokio::fs — a multi-GB drag-drop must not block one of the 4 async workers.
            let data = tokio::fs::read(&pb).await.map_err(|x| format!("read {}: {x}", pb.display()))?;
            let filename = pb.file_name().and_then(|f| f.to_str()).unwrap_or("file").to_string();
            let mime: mime::Mime = e.mime.parse().unwrap_or(mime::APPLICATION_OCTET_STREAM);
            // docs/PHOTO-METADATA.md: an image goes up without its location + identifying metadata (lossless, the
            // orientation kept); one that can't be cleaned is not sent. The event states the cleaned size.
            let data = photo_bytes_for_upload(mime.as_ref(), data)?;
            let size = data.len() as u64;
            // Our own upload + send_raw instead of Room::send_attachment, which reads the encryption state once, before
            // the upload: a room that turned encrypted during it got an encrypted event pointing at plaintext media.
            let source = attach_upload_checked(
                || async { room.latest_encryption_state().await.map_err(|x| x.to_string()) },
                || room.encryption_state(),
                |encrypted| {
                    let (client, data, mime) = (&client, &data, &mime);
                    async move {
                        if encrypted {
                            let mut cur = std::io::Cursor::new(data.as_slice());
                            let f = client.upload_encrypted_file(&mut cur).await.map_err(|x| x.to_string())?;
                            Ok(MediaSource::Encrypted(Box::new(f)))
                        } else {
                            let r = client.media().upload(mime, data.clone(), None).await.map_err(|x| x.to_string())?;
                            Ok(MediaSource::Plain(r.content_uri))
                        }
                    }
                },
            )
            .await?;
            let content = attachment_content(&mime, filename, source, e.w, e.h, size);
            let mut content = serde_json::to_value(content).map_err(|x| x.to_string())?;
            // threads: an attachment from a thread's panel goes into the thread
            if let Some(rel) = echo_relation(e) {
                content["m.relates_to"] = rel;
            }
            // send_raw encrypts in an E2EE room. The txn is the echo's, so a send that did reach the server is not doubled by a retry.
            let r = room
                .send_raw("m.room.message", content)
                .with_transaction_id(&txn)
                .await
                .map_err(|x| x.to_string())?;
            Ok(r.response.event_id.to_string())
        }
        EchoKind::Voice(v) => {
            use tinhat_widgets::voicenote::{voice_content, VoiceMedia};
            let client = room.client();
            // review r1 #5 / r2 #4: the draft's send token (None = discarded already); a discarded draft (leave,
            // sign-out, Delete, another account) stops here after every await, and send_raw races the token
            let token = crate::voicenote_ctl::send_token(&e.txn).ok_or_else(|| "voice: discarded".to_string())?;
            let alive = |what: &str| -> Result<(), String> {
                if !token.is_cancelled() && crate::voicenote_ctl::send_allowed(&e.txn) {
                    Ok(())
                } else {
                    eprintln!("[voicenote] send of {} stopped ({what}): the draft was discarded", e.txn);
                    Err("voice: discarded".into())
                }
            };
            alive("start")?;
            // review r1 #1: the room's encryption is read at send time (and again right before the event goes out): a
            // plaintext upload cached from an earlier try is never reused once the room is encrypted — the note is
            // uploaded again, encrypted, under the same transaction id
            let mut tries = 0;
            let (media, size) = loop {
                tries += 1;
                let at_send = room.latest_encryption_state().await.map_err(|x| format!("voice: {x}"))?;
                // (Unknown after asking = can't tell: nothing is uploaded — voice_upload_encrypt)
                let encrypted = voice_upload_encrypt(&at_send, &at_send)?;
                alive("encryption state")?;
                let cached = voice_upload_get(&e.txn).filter(|(m, _)| voice_upload_fits(m, encrypted));
                let (media, size) = match cached {
                    Some(m) => m,
                    None => {
                        let data = tokio::fs::read(&v.path).await.map_err(|x| format!("voice: read {}: {x}", v.path))?;
                        // R3-1: a voice note is never a picture: bytes that sniff as one are not sent
                        refuse_picture_bytes(&data).map_err(|x| format!("voice: {x}"))?;
                        alive("read")?;
                        let size = data.len() as u64;
                        // §6: the server's m.upload.size (cached by the SDK); unknown = no refusal
                        let limit = client.load_or_fetch_max_upload_size().await.ok().map(u64::from);
                        alive("media config")?;
                        if voice_over_limit(size, limit) {
                            return Err(format!("voice: M_TOO_LARGE ({size} bytes over the server's {} byte limit)", limit.unwrap_or(0)));
                        }
                        // VN-2: read again right before the upload starts (no await from here to the request)
                        let encrypted = voice_upload_encrypt(&at_send, &room.encryption_state())?;
                        let media = if encrypted {
                            // AES-CTR-256 + SHA-256 (the SDK's attachment path): the event carries `file`, never `url`
                            let mut cur = std::io::Cursor::new(data);
                            let f = client.upload_encrypted_file(&mut cur).await.map_err(|x| format!("voice: {x}"))?;
                            VoiceMedia::File(serde_json::to_value(&f).map_err(|x| format!("voice: {x}"))?)
                        } else {
                            let mime: mime::Mime = "audio/ogg".parse().map_err(|_| "voice: mime".to_string())?;
                            let r = client.media().upload(&mime, data, None).await.map_err(|x| format!("voice: {x}"))?;
                            VoiceMedia::Url(r.content_uri.to_string())
                        };
                        // cached only while the draft may still send — checked and inserted under the drafts' lock, so a
                        // discard can never slip in between and leave this upload behind
                        if !crate::voicenote_ctl::upload_put_if_alive(&e.txn, media.clone(), size) {
                            alive("upload")?;
                            return Err("voice: discarded".into());
                        }
                        (media, size)
                    }
                };
                // the room turned encrypted while a plaintext note was uploading: once more, encrypted
                let now_state = room.latest_encryption_state().await.map_err(|x| format!("voice: {x}"))?;
                let now = voice_upload_encrypt(&now_state, &room.encryption_state())?;
                alive("encryption recheck")?;
                if voice_upload_fits(&media, now) || tries >= 3 {
                    if !voice_upload_fits(&media, now) {
                        return Err("voice: the room's encryption changed during the upload".into());
                    }
                    break (media, size);
                }
            };
            let content = voice_content(&media, "audio/ogg", size, v.duration_ms, &v.waveform);
            alive("send")?;
            // send_raw encrypts in an E2EE room (matrix-sdk: SendRawMessageLikeEvent checks the room's encryption state).
            // Its preparation (room keys) and request race the draft's token: a discard stops it there too. The txn is
            // the draft's, so a send that did reach the server is never doubled by a retry.
            let send = std::future::IntoFuture::into_future(room.send_raw("m.room.message", content).with_transaction_id(&txn));
            // review r3 #3: `biased`, the token FIRST — a discard that is already in is seen before the send is polled
            // (again); a request already on the wire cannot be called back, which the txn makes safe to retry
            let r = tokio::select! {
                biased;
                _ = token.cancelled() => {
                    eprintln!("[voicenote] send of {} stopped (in send_raw): the draft was discarded", e.txn);
                    return Err("voice: discarded".into());
                }
                r = send => r.map_err(|x| format!("voice: {x}"))?,
            };
            voice_upload_forget(&e.txn);
            Ok(r.response.event_id.to_string())
        }
    }
}

/// May the file go up, and encrypted? Encrypted when either read says so; a state the SDK can't tell (Unknown) uploads
/// nothing. Rooms cannot turn encryption off, so "either" is the safe side.
pub(crate) fn attach_upload_encrypt(at_send: &matrix_sdk::EncryptionState, at_upload: &matrix_sdk::EncryptionState) -> Result<bool, String> {
    if at_send.is_unknown() || at_upload.is_unknown() {
        return Err("attachment: the room's encryption can't be checked, so nothing was sent".into());
    }
    Ok(at_send.is_encrypted() || at_upload.is_encrypted())
}

/// Upload an attachment so its media matches the room's encryption when the upload ends: `latest` asks (the server only
/// while the state is unknown), `current` reads synchronously right before the upload starts, `upload(encrypted)` does
/// the upload. A plaintext upload is re-checked afterwards and done again, encrypted, when the room turned encrypted
/// meanwhile. Sync runs on other tasks, so a change landing between the last check and send_raw is narrowed, not excluded.
pub(crate) async fn attach_upload_checked<T, L, LF, U, UF>(
    mut latest: L,
    current: impl Fn() -> matrix_sdk::EncryptionState,
    mut upload: U,
) -> Result<T, String>
where
    L: FnMut() -> LF,
    LF: std::future::Future<Output = Result<matrix_sdk::EncryptionState, String>>,
    U: FnMut(bool) -> UF,
    UF: std::future::Future<Output = Result<T, String>>,
{
    for _ in 0..3 {
        let at_send = latest().await?;
        attach_upload_encrypt(&at_send, &at_send)?;
        // no await from this read to the start of the upload
        let encrypted = attach_upload_encrypt(&at_send, &current())?;
        let media = upload(encrypted).await?;
        if encrypted {
            return Ok(media); // encrypted media is fine in any room
        }
        let now = latest().await?;
        if !attach_upload_encrypt(&now, &current())? {
            return Ok(media);
        }
    }
    Err("attachment: the room's encryption changed during the upload".into())
}

/// The m.room.message for an uploaded attachment: the same content Room::send_attachment built. An image carries its
/// pixel size and byte size when known (receivers reserve the row's height before the bytes arrive).
pub(crate) fn attachment_content(
    mime: &mime::Mime,
    filename: String,
    source: matrix_sdk::ruma::events::room::MediaSource,
    w: u32,
    h: u32,
    size: u64,
) -> RoomMessageEventContent {
    use matrix_sdk::attachment::{AttachmentInfo, BaseImageInfo};
    use matrix_sdk::ruma::events::room::message::{AudioInfo, AudioMessageEventContent, FileInfo, FileMessageEventContent, ImageMessageEventContent, MessageType, VideoInfo, VideoMessageEventContent};
    use matrix_sdk::ruma::events::room::ImageInfo;
    let mimetype = Some(mime.as_ref().to_owned());
    let msg = match mime.type_() {
        mime::IMAGE => {
            let mut info = if w > 0 && h > 0 {
                ImageInfo::from(AttachmentInfo::Image(BaseImageInfo {
                    width: Some(UInt::from(w)),
                    height: Some(UInt::from(h)),
                    size: UInt::new(size),
                    ..Default::default()
                }))
            } else {
                ImageInfo::default()
            };
            info.mimetype = mimetype;
            let mut c = ImageMessageEventContent::new(filename, source);
            c.info = Some(Box::new(info));
            MessageType::Image(c)
        }
        mime::AUDIO => {
            let mut info = AudioInfo::default();
            info.mimetype = mimetype;
            MessageType::Audio(AudioMessageEventContent::new(filename, source).info(Box::new(info)))
        }
        mime::VIDEO => {
            let mut info = VideoInfo::default();
            info.mimetype = mimetype;
            let mut c = VideoMessageEventContent::new(filename, source);
            c.info = Some(Box::new(info));
            MessageType::Video(c)
        }
        _ => {
            let mut info = FileInfo::default();
            info.mimetype = mimetype;
            let mut c = FileMessageEventContent::new(filename, source);
            c.info = Some(Box::new(info));
            MessageType::File(c)
        }
    };
    RoomMessageEventContent::new(msg)
}

/// The words when a photo's metadata couldn't be removed (nothing was uploaded). The web's PHOTO_LOCATION_FAIL.
pub(crate) const PHOTO_LOCATION_FAIL: &str = "Couldn't remove this photo's location, so it wasn't sent";
const PHOTO_LOCATION_TAG: &str = "photo location: ";

/// "Location is removed from photos you send" (docs/PHOTO-METADATA.md): the bytes to upload for a file of type `mime`.
/// An image loses its GPS and every identifying tag, losslessly, keeping only the EXIF orientation
/// (tinhat_widgets::exifstrip, the shared vectors testvectors/exif-strip); anything that isn't an image goes up exactly
/// as it is. Err = the image couldn't be cleaned: the caller must not upload it. Every image upload calls this: the chat
/// attachment (attach, paste, drag-drop), the room photo, the profile photo (settings.rs) and Memories "Add photos".
/// The web's core/photoUpload.ts photoBytesForUpload.
/// 12.1.16 2-2: the BYTES decide (tinhat_widgets::exifstrip::sniff_image), never `_mime` or the file's extension: a
/// JPEG/PNG/WebP/HEIC/HEIF/AVIF is cleaned (or refused) whatever it is called; a TIFF (DNG and the camera RAWs built
/// on it) is refused: it carries Exif/GPS and isn't cleaned (launch-gate 13:55); anything else, GIF included (the only
/// pass-through picture format), goes up exactly as it is. Shared table: testvectors/exif-strip/upload-rule.json.
/// 12.1.16 R3-1: an upload that is NOT a picture (a soundboard clip, a voice note) refuses bytes that sniff as a picture
/// in ANY format (GIF, TIFF and RAWs included): such a path never sends pictures, and a picture renamed `clip.mp3` could
/// carry a location. Nothing is cleaned; Err = nothing is uploaded (the words: PHOTO_LOCATION_FAIL). The web's
/// core/photoUpload.ts refusePictureBytes. The guard every_image_upload_is_covered names every such site.
pub(crate) fn refuse_picture_bytes(data: &[u8]) -> Result<(), String> {
    match tinhat_widgets::exifstrip::sniff_image(data) {
        Some(kind) => Err(format!("{PHOTO_LOCATION_TAG}{kind}: a picture on a non-picture upload, not sent")),
        None => Ok(()),
    }
}

pub(crate) fn photo_bytes_for_upload(_mime: &str, data: Vec<u8>) -> Result<Vec<u8>, String> {
    if let Some(kind @ ("tiff" | "raw")) = tinhat_widgets::exifstrip::sniff_image(&data) {
        return Err(format!("{PHOTO_LOCATION_TAG}{kind}: not cleaned, not sent"));
    }
    if !tinhat_widgets::exifstrip::is_cleaned_picture(&data) {
        return Ok(data);
    }
    match tinhat_widgets::exifstrip::strip_image_metadata(&data) {
        // the sniff says picture, the cleaner doesn't recognise it (e.g. a HEIC cut inside its ftyp): fail closed
        Ok(s) if s.format == tinhat_widgets::exifstrip::StripFormat::Other => Err(format!("{PHOTO_LOCATION_TAG}not a picture the cleaner can read")),
        Ok(s) if s.bytes == data => Ok(data),
        Ok(s) => Ok(s.bytes),
        Err(e) => Err(format!("{PHOTO_LOCATION_TAG}{e}")),
    }
}

/// L1: a failed request's class from the SDK's error text (forbidden | too-large | rate | network | other); the words
/// for it are the view's failText (ConversationView.qml composer-draft pure block). Tin Hat Web's failClass
/// (src/features/chat/lib/composerDraft.ts) follows the same rules; both replay testvectors/composer-draft "classes".
pub(crate) fn fail_class(raw: &str) -> &'static str {
    // a photo that couldn't be cleaned is never a network failure, whatever its error says: it never resends by itself
    // (the assembly's note; the vectors' "photo location: …" class, the web's ExifStripError)
    if raw.starts_with(PHOTO_LOCATION_TAG) || raw.contains(PHOTO_LOCATION_FAIL) {
        return "other";
    }
    let l = raw.to_lowercase();
    if l.contains("m_forbidden") || l.contains("403") {
        "forbidden"
    } else if l.contains("m_too_large") || l.contains("413") {
        "too-large"
    } else if l.contains("m_limit_exceeded") || l.contains("429") {
        "rate"
    } else if ["connect", "timed out", "timeout", "dns", "network", "fetch"].iter().any(|w| l.contains(w)) {
        "network"
    } else {
        "other"
    }
}

/// L1: append a failure record (JSON) to a list of them (JSON array; anything else = empty), keeping the last 8 — two
/// failures between the view's polls are both seen (astra review: one slot lost the first).
/// The record's seq is set HERE, on the Qt thread, one past the list's newest: the tasks that fail can queue out of
/// order, and the view handles only seqs above the newest it has handled (astra review round 2).
pub(crate) fn push_failure(list_json: &str, rec: &str) -> String {
    let mut v: Vec<serde_json::Value> = serde_json::from_str(list_json).unwrap_or_default();
    if let Ok(mut r) = serde_json::from_str::<serde_json::Value>(rec) {
        let next = v.iter().filter_map(|x| x["seq"].as_u64()).max().unwrap_or(0) + 1;
        r["seq"] = serde_json::json!(next);
        v.push(r);
    }
    let skip = v.len().saturating_sub(8);
    serde_json::Value::Array(v.into_iter().skip(skip).collect()).to_string()
}

static OP_FAILED_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The opFailedJson record of a refused edit / delete (a new seq each time, so the same failure twice is seen twice).
fn op_failed(room_id: &str, kind: &str, id: &str, text: &str, err: &str) -> String {
    let seq = OP_FAILED_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    serde_json::json!({ "seq": seq, "room": room_id, "kind": kind, "id": id, "text": text, "cls": fail_class(err) }).to_string()
}

#[cfg(test)]
mod resend_photo_tests {
    // the assembly's note: L1's auto-resend never resends a photo that couldn't be cleaned
    #[test]
    fn a_photo_that_could_not_be_cleaned_never_resends_by_itself() {
        for err in [
            "photo location: connection reset while reading the image",
            "photo location: timed out decoding the tiff block",
            super::PHOTO_LOCATION_FAIL,
        ] {
            assert_eq!(super::fail_class(err), "other", "{err}");
            assert!(!super::resendable(super::fail_class(err), false, 0), "{err}");
        }
        // what the cleaner itself returns for a broken image goes the same way
        if let Err(broken) = super::photo_bytes_for_upload("image/jpeg", vec![0xFF, 0xD8, 0xFF, 0xE1, 0x00, 0x08, 0x45]) {
            assert!(!super::resendable(super::fail_class(&broken), false, 0), "{broken}");
        }
    }

    #[test]
    fn a_resend_goes_through_the_cleaner_before_any_upload() {
        let src = include_str!("room.rs");
        let r = src.find("pub(crate) fn resend_after_reconnect()").unwrap();
        let rbody = &src[r..r + src[r..].find("\n}\n").unwrap()];
        assert!(rbody.contains("perform_echo(&c, &rid, &e)"), "a resend is an ordinary send (perform_echo)");
        let f = src.find("async fn perform_echo(").unwrap();
        let body = &src[f..f + src[f..].find("\n}\n").unwrap()];
        let arm = &body[body.find("EchoKind::Attachment(path) => {").unwrap()..];
        let clean = arm.find("photo_bytes_for_upload(mime.as_ref(), data)?").expect("the attachment is cleaned");
        let up = arm.find("attach_upload_checked(").expect("the upload");
        assert!(clean < up, "cleaned before it is uploaded; a failure (?) returns before any upload");
    }
}

#[cfg(test)]
mod resend_session_tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    fn echo(txn: &str) -> super::Echo {
        let mut e = super::Echo::new(super::EchoKind::Text, txn);
        e.txn = txn.into();
        e
    }

    // 4-3: the session ends (sign-out / another account) while the first resend is in flight: nothing after it is sent.
    #[test]
    fn a_sign_out_stops_the_batch() {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let gen = Arc::new(AtomicU64::new(7));
        let sent = Arc::new(Mutex::new(Vec::<String>::new()));
        let (g2, s2) = (gen.clone(), sent.clone());
        let n = rt.block_on(super::resend_batch(7, move || gen.load(Ordering::SeqCst), "!r:x", vec![echo("a"), echo("b"), echo("c")], move |e| {
            s2.lock().unwrap().push(e.txn.clone());
            g2.store(8, Ordering::SeqCst); // signed out while "a" was on its way
            async { Ok::<String, String>("$a".into()) }
        }));
        assert_eq!(*sent.lock().unwrap(), vec!["a".to_string()], "b and c must not go out as the next session");
        assert_eq!(n, 0, "the answer of an ended session is not recorded");
    }

    #[test]
    fn the_same_session_sends_them_all_in_order() {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let sent = Arc::new(Mutex::new(Vec::<String>::new()));
        let s2 = sent.clone();
        let n = rt.block_on(super::resend_batch(3, || 3, "!r:x", vec![echo("a"), echo("b")], move |e| {
            s2.lock().unwrap().push(e.txn.clone());
            async { Ok::<String, String>("$x".into()) }
        }));
        assert_eq!((n, sent.lock().unwrap().clone()), (2, vec!["a".to_string(), "b".to_string()]));
    }

    #[test]
    fn a_batch_never_finds_its_room_through_the_global_client() {
        let src = include_str!("room.rs");
        let f = src.find("async fn perform_echo(").unwrap();
        let body = &src[f..f + src[f..].find("\n}\n").unwrap()];
        assert!(body.starts_with("async fn perform_echo(client: &matrix_sdk::Client,"), "perform_echo takes the session's client");
        assert!(!body.contains("room_by_id(") && !body.contains("session::client()"), "no lookup of the CURRENT client inside a send");
        let r = src.find("pub(crate) fn resend_after_reconnect()").unwrap();
        let rbody = &src[r..r + src[r..].find("\n}\n").unwrap()];
        assert!(rbody.contains("let gen = crate::session::generation();") && rbody.contains("resend_batch(gen, crate::session::generation,"));
    }
}

#[cfg(test)]
mod resend_tests {
    #[test]
    fn shared_vectors() {
        let v: serde_json::Value = serde_json::from_str(include_str!("../../../testvectors/resend-after-reconnect/vectors.json")).unwrap();
        let cases = v["cases"].as_array().unwrap();
        assert!(cases.len() >= 30);
        for c in cases {
            let (cls, kind, tries) = (c["class"].as_str().unwrap(), c["kind"].as_str().unwrap(), c["tries"].as_u64().unwrap() as u8);
            assert_eq!(super::resendable(cls, kind == "voice", tries), c["resend"].as_bool().unwrap(), "{cls} {kind} {tries}");
        }
        assert_eq!(super::MAX_AUTO_TRIES as u64, v["maxAutoTries"].as_u64().unwrap());
        // what a network-failed message says while it waits
        assert_eq!(super::friendly_send_error("error sending request: connection refused"), v["networkText"].as_str().unwrap());
    }

    #[test]
    fn a_network_failure_is_marked_resendable_and_resent_pending() {
        let room = "!resend-test:x";
        let mut e = super::Echo::new(super::EchoKind::Text, "hi");
        e.txn = "t-resend-1".into();
        super::echo_store_push(room, e);
        super::echo_store_failed(room, "t-resend-1", "error sending request: connection refused");
        let mut f = super::Echo::new(super::EchoKind::Text, "no");
        f.txn = "t-resend-2".into();
        super::echo_store_push(room, f);
        super::echo_store_failed(room, "t-resend-2", "M_FORBIDDEN");
        {
            let s = super::echo_store().lock().unwrap();
            let v = s.get(room).unwrap();
            assert_eq!((v[0].fail_cls, v[1].fail_cls), ("network", "forbidden"));
        }
        // (no client in a unit test: the send itself fails again; what matters is which ones were picked up)
        let picked: Vec<String> = {
            let s = super::echo_store().lock().unwrap();
            s.get(room).unwrap().iter().filter(|e| e.failed.is_some() && super::resendable(e.fail_cls, false, e.auto_tries)).map(|e| e.txn.clone()).collect()
        };
        assert_eq!(picked, vec!["t-resend-1".to_string()]);
        let _ = super::echo_store_take(room, "t-resend-1");
        let _ = super::echo_store_take(room, "t-resend-2");
    }
}

#[cfg(test)]
mod fail_class_tests {
    #[test]
    fn shared_vectors_classes() {
        let v: serde_json::Value = serde_json::from_str(include_str!("../../../testvectors/composer-draft/vectors.json")).unwrap();
        let cases = v["classes"].as_array().unwrap();
        assert!(cases.len() >= 10);
        for c in cases {
            assert_eq!(super::fail_class(c["error"].as_str().unwrap()), c["class"].as_str().unwrap(), "{}", c["error"]);
        }
    }

    #[test]
    fn op_failed_record() {
        let a: serde_json::Value = serde_json::from_str(&super::op_failed("!r:x", "edit", "$e", "fixed", "error sending request: connection refused")).unwrap();
        let b: serde_json::Value = serde_json::from_str(&super::op_failed("!r:x", "edit", "$e", "fixed", "M_FORBIDDEN")).unwrap();
        assert_eq!((a["kind"].as_str(), a["id"].as_str(), a["text"].as_str(), a["cls"].as_str()), (Some("edit"), Some("$e"), Some("fixed"), Some("network")));
        assert_eq!(b["cls"].as_str(), Some("forbidden"));
        assert!(b["seq"].as_u64() > a["seq"].as_u64());
    }

    #[test]
    fn failures_queue_up_to_eight() {
        let mut list = String::new();
        // the seq a task stamped doesn't count (tasks queue out of order): the list numbers its records in order
        for i in [5u64, 3, 9, 1, 2, 8, 7, 4, 6, 0] {
            list = super::push_failure(&list, &format!(r#"{{"seq":{i},"n":{i}}}"#));
        }
        let v: Vec<serde_json::Value> = serde_json::from_str(&list).unwrap();
        assert_eq!(v.len(), 8);
        let seqs: Vec<u64> = v.iter().map(|x| x["seq"].as_u64().unwrap()).collect();
        assert_eq!(seqs, (3..=10).collect::<Vec<u64>>());
        assert_eq!(v[7]["n"].as_u64(), Some(0), "newest last");
    }
}

/// Short, human wording for a failed send (the raw SDK error is logged, not shown).
fn friendly_send_error(raw: &str) -> String {
    if raw.starts_with(PHOTO_LOCATION_TAG) {
        return PHOTO_LOCATION_FAIL.into();
    }
    // voice notes: the note's own words for "too large" (§6); everything else as for any send
    if let Some(r) = raw.strip_prefix("voice: ") {
        let l = r.to_lowercase();
        if l.contains("m_too_large") || l.contains("413") {
            return "This voice message is too large for the server".into();
        }
        return friendly_send_error(r);
    }
    let l = raw.to_lowercase();
    if l.contains("m_forbidden") || l.contains("403") {
        "You don't have permission to send here".into()
    } else if l.contains("m_too_large") || l.contains("413") {
        "That file is too large for the server".into()
    } else if l.contains("m_limit_exceeded") || l.contains("429") {
        "Slow down — the server is rate-limiting sends".into()
    } else if l.contains("connect") || l.contains("timed out") || l.contains("timeout") || l.contains("dns") || l.contains("network") {
        // L1 D08.2: it does now (resend_after_reconnect)
        "No connection — it will send when you're back online".into()
    } else {
        "Couldn't send".into()
    }
}

/// Apply `f` to the row whose "id" is `id`; the new JSON, or None when nothing matched.
fn patch_row(json: &str, id: &str, f: impl FnOnce(&mut serde_json::Value)) -> Option<String> {
    let mut v: serde_json::Value = serde_json::from_str(json).ok()?;
    let arr = v.as_array_mut()?;
    let m = arr.iter_mut().find(|m| m.get("id").and_then(|i| i.as_str()) == Some(id))?;
    f(m);
    serde_json::to_string(&v).ok()
}

/// Replace the row with `row["id"]` or append it (an echo painted for the first time).
fn upsert_row(json: &str, row: serde_json::Value) -> String {
    let id = row.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();
    let mut v: serde_json::Value = serde_json::from_str(json).unwrap_or_else(|_| serde_json::json!([]));
    if !v.is_array() {
        v = serde_json::json!([]);
    }
    let arr = v.as_array_mut().unwrap();
    if let Some(m) = arr.iter_mut().find(|m| m.get("id").and_then(|i| i.as_str()) == Some(id.as_str())) {
        *m = row;
    } else {
        arr.push(row);
    }
    serde_json::to_string(&v).unwrap_or_else(|_| "[]".into())
}

fn remove_row(json: &str, id: &str) -> Option<String> {
    let mut v: serde_json::Value = serde_json::from_str(json).ok()?;
    let arr = v.as_array_mut()?;
    let n = arr.len();
    arr.retain(|m| m.get("id").and_then(|i| i.as_str()) != Some(id));
    if arr.len() == n {
        return None;
    }
    serde_json::to_string(&v).ok()
}

/// Sentinel reactionId for a chip we've added optimistically but the server hasn't yet
/// confirmed. Chips with this id are removed locally on unreact (no server redact) and
/// are replaced wholesale when the next sync rebuilds reactions from server events.
const OPTIMISTIC_RID: &str = "__optimistic__";

/// Add (`add = true`) or remove (`add = false`) OUR reaction `key` on the message whose
/// id is `event_id`, mutating the timeline `messages_json` in place. Returns the new JSON
/// string, or `None` if nothing changed (parse failure / target/chip not found).
///
/// De-dupe guard: a chip already marked `mine` is never double-incremented on add, and a
/// chip not marked `mine` is left alone on remove — so an optimistic add followed by the
/// real m.reaction (which triggers a full rebuild, not another apply) can't double-count.
fn apply_optimistic_react(json: &str, event_id: &str, key: &str, add: bool) -> Option<String> {
    let mut msgs: Vec<serde_json::Value> = serde_json::from_str(json).ok()?;
    let mut changed = false;
    for m in msgs.iter_mut() {
        if m.get("id").and_then(|i| i.as_str()) != Some(event_id) {
            continue;
        }
        let arr = m
            .get_mut("reactions")
            .and_then(|r| r.as_array_mut())
            .map(std::mem::take);
        let mut chips = arr.unwrap_or_default();
        // QOL A07.2: ❤ and ❤️ are one chip
        let nkey = crate::reactions::react_key_norm(key);
        let existing = chips
            .iter()
            .position(|c| c.get("key").and_then(|k| k.as_str()).map(crate::reactions::react_key_norm).as_deref() == Some(nkey.as_str()));
        if add {
            match existing {
                Some(i) => {
                    let mine = chips[i].get("mine").and_then(|b| b.as_bool()).unwrap_or(false);
                    if !mine {
                        let count = chips[i].get("count").and_then(|c| c.as_u64()).unwrap_or(1) + 1;
                        chips[i]["count"] = serde_json::json!(count);
                        chips[i]["mine"] = serde_json::json!(true);
                        chips[i]["reactionId"] = serde_json::json!(OPTIMISTIC_RID);
                        chips[i]["reactionIds"] = serde_json::json!([]); // QOL A07.2: my server ids are not this chip's now
                        changed = true;
                    }
                }
                None => {
                    chips.push(serde_json::json!({
                        "key": key, "count": 1, "mine": true, "reactionId": OPTIMISTIC_RID
                    }));
                    changed = true;
                }
            }
        } else if let Some(i) = existing {
            let mine = chips[i].get("mine").and_then(|b| b.as_bool()).unwrap_or(false);
            if mine {
                let count = chips[i].get("count").and_then(|c| c.as_u64()).unwrap_or(1);
                if count <= 1 {
                    chips.remove(i);
                } else {
                    chips[i]["count"] = serde_json::json!(count - 1);
                    chips[i]["mine"] = serde_json::json!(false);
                    chips[i]["reactionId"] = serde_json::json!("");
                    chips[i]["reactionIds"] = serde_json::json!([]);
                }
                changed = true;
            }
        }
        m["reactions"] = serde_json::json!(chips);
        break;
    }
    if changed {
        serde_json::to_string(&msgs).ok()
    } else {
        None
    }
}

/// Optimistically remove the chip identified by `reaction_id` (our own reaction). Returns
/// the new JSON plus the (target_event_id, key) we removed, so a failed redact can re-add
/// it. `None` if the chip isn't found.
fn remove_optimistic_react(json: &str, reaction_id: &str) -> Option<(String, String, String)> {
    let mut msgs: Vec<serde_json::Value> = serde_json::from_str(json).ok()?;
    for m in msgs.iter_mut() {
        let target = m.get("id").and_then(|i| i.as_str()).map(String::from);
        let Some(arr) = m.get_mut("reactions").and_then(|r| r.as_array_mut()) else {
            continue;
        };
        let Some(i) = arr
            .iter()
            .position(|c| c.get("reactionId").and_then(|r| r.as_str()) == Some(reaction_id))
        else {
            continue;
        };
        let key = arr[i]
            .get("key")
            .and_then(|k| k.as_str())
            .unwrap_or("")
            .to_string();
        let count = arr[i].get("count").and_then(|c| c.as_u64()).unwrap_or(1);
        if count <= 1 {
            arr.remove(i);
        } else {
            arr[i]["count"] = serde_json::json!(count - 1);
            arr[i]["mine"] = serde_json::json!(false);
            arr[i]["reactionId"] = serde_json::json!("");
            arr[i]["reactionIds"] = serde_json::json!([]);
        }
        let out = serde_json::to_string(&msgs).ok()?;
        return Some((out, target.unwrap_or_default(), key));
    }
    None
}

async fn do_react(room_id: &str, event_id: &str, key: &str) -> Result<(), String> {
    use matrix_sdk::ruma::events::reaction::ReactionEventContent;
    use matrix_sdk::ruma::events::relation::Annotation;
    let client = crate::session::client().ok_or_else(|| "Not signed in".to_string())?;
    let rid = RoomId::parse(room_id).map_err(|e| format!("bad room id: {e}"))?;
    let room = client.get_room(&rid).ok_or_else(|| "Room not found".to_string())?;
    let target = matrix_sdk::ruma::EventId::parse(event_id).map_err(|e| format!("bad event id: {e}"))?;
    let content = ReactionEventContent::new(Annotation::new(target, key.to_string()));
    room.send(content).await.map_err(|e| e.to_string())?;
    Ok(())
}

async fn do_unreact(room_id: &str, reaction_id: &str) -> Result<(), String> {
    let client = crate::session::client().ok_or_else(|| "Not signed in".to_string())?;
    let rid = RoomId::parse(room_id).map_err(|e| format!("bad room id: {e}"))?;
    let room = client.get_room(&rid).ok_or_else(|| "Room not found".to_string())?;
    let eid = matrix_sdk::ruma::EventId::parse(reaction_id).map_err(|e| format!("bad event id: {e}"))?;
    room.redact(&eid, None, None).await.map_err(|e| e.to_string())?;
    Ok(())
}

/// Default timeline window (messages) shown on open; grows by WINDOW_STEP via load_older.
const DEFAULT_WINDOW: usize = 50;
const WINDOW_STEP: usize = 50;

/// A35.4: the window depth once the reader is back at the newest: the default again (load_older grows it a page at a
/// time; only a room open used to shrink it, so after a deep scroll-back every live pass reloaded the whole depth).
fn window_after_jump_to_latest(_current: usize) -> usize {
    DEFAULT_WINDOW
}

#[cfg(test)]
mod window_tests {
    use super::{window_after_jump_to_latest, DEFAULT_WINDOW, WINDOW_STEP};

    #[test]
    fn jumping_to_the_latest_shrinks_a_deep_window_back_to_the_default() {
        assert_eq!(window_after_jump_to_latest(DEFAULT_WINDOW + 12 * WINDOW_STEP), DEFAULT_WINDOW);
        assert_eq!(window_after_jump_to_latest(DEFAULT_WINDOW), DEFAULT_WINDOW);
    }
}
/// Compute read receipts only for the most recent N messages (store reads on the hot path).
const RECEIPT_TAIL: usize = 30;
/// Debounce before a live-update reload — bursts of sync updates collapse into one
/// /messages refetch.
const RELOAD_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(250);
/// Minimum spacing between reloads triggered ONLY by read receipts (receipt churn in a
/// busy room otherwise refetches the whole window on every sync).
const RECEIPT_COALESCE: std::time::Duration = std::time::Duration::from_secs(2);

/// 10.5.2 (timeline): does a background result still belong to what the controller shows? The
/// room-id stamp alone was not enough: a load started on an EARLIER visit of the same room
/// (A → B → A) passed it, landed after the new open, cleared `busy` early (freezing the NEW
/// divider on the snapshot) and could repaint older data over newer.
fn same_open(cur_gen: u64, cur_room: &str, gen: u64, room_id: &str) -> bool {
    cur_gen == gen && cur_room == room_id
}

fn is_current(o: &qobject::RoomController, gen: u64, room_id: &str) -> bool {
    same_open(o.rust().open_gen, &o.room_id().to_string(), gen, room_id)
}

/// 10.5.2: the wait before re-trying a failed timeline load — 2 s, then doubling, at most 30 s.
fn retry_backoff(fail_streak: u32) -> std::time::Duration {
    let doublings = fail_streak.saturating_sub(1).min(4);
    std::time::Duration::from_secs((2u64 << doublings).min(30))
}

/// 10.5.2: per-room "new megolm keys arrived" counters. Bumped by the SDK's room-key stream (a late
/// key share, a forwarded key, a backup import) and by our own backup download; a window whose
/// epoch is behind re-tries its undecryptable events (see `redecrypt_window`).
fn keys_epochs() -> &'static std::sync::Mutex<std::collections::HashMap<String, u64>> {
    static E: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, u64>>> =
        std::sync::OnceLock::new();
    E.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}
/// Bumped when the key stream lagged (we can't tell which rooms got keys: every window retries once).
static KEYS_EPOCH_ALL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn bump_keys_epoch(room_id: &str) {
    if let Ok(mut m) = keys_epochs().lock() {
        *m.entry(room_id.to_owned()).or_insert(0) += 1;
    }
}

fn keys_epoch(room_id: &str) -> u64 {
    let own = keys_epochs().lock().ok().and_then(|m| m.get(room_id).copied()).unwrap_or(0);
    KEYS_EPOCH_ALL.load(Ordering::Relaxed).wrapping_add(own)
}

/// Room ids that just received megolm keys ("*" = unknown, the stream lagged). The open room's live
/// loop listens, so rows that become readable repaint without waiting for the next room event.
fn keys_channel() -> &'static tokio::sync::broadcast::Sender<String> {
    static C: std::sync::OnceLock<tokio::sync::broadcast::Sender<String>> = std::sync::OnceLock::new();
    C.get_or_init(|| tokio::sync::broadcast::channel(256).0)
}

/// One listener on the SDK's room-key stream per SESSION (user + device): a sign-out / sign-in in the
/// same process starts a new one and the old one stops at its next item. Restarted by the next
/// open() if it ends (a regenerated olm machine).
fn watch_room_keys(client: &matrix_sdk::Client) {
    static WATCHED: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
    let who = format!(
        "{}|{}",
        client.user_id().map(|u| u.as_str()).unwrap_or(""),
        client.device_id().map(|d| d.as_str()).unwrap_or("")
    );
    {
        let mut w = WATCHED.lock().unwrap_or_else(|p| p.into_inner());
        if w.as_deref() == Some(who.as_str()) {
            return;
        }
        *w = Some(who.clone());
    }
    let client = client.clone();
    crate::bridge::runtime().spawn(async move {
        use futures_util::StreamExt;
        let current = |who: &str| WATCHED.lock().map(|w| w.as_deref() == Some(who)).unwrap_or(false);
        if let Some(stream) = client.encryption().room_keys_received_stream().await {
            let mut stream = Box::pin(stream);
            while let Some(update) = stream.next().await {
                if !current(&who) {
                    return; // a newer session's watcher took over
                }
                match update {
                    Ok(infos) => {
                        let mut rooms: Vec<String> = infos.iter().map(|i| i.room_id.to_string()).collect();
                        rooms.sort_unstable();
                        rooms.dedup();
                        for r in rooms {
                            bump_keys_epoch(&r);
                            let _ = keys_channel().send(r);
                        }
                    }
                    Err(_) => {
                        KEYS_EPOCH_ALL.fetch_add(1, Ordering::Relaxed);
                        let _ = keys_channel().send("*".to_owned());
                    }
                }
            }
        }
        if let Ok(mut w) = WATCHED.lock() {
            if w.as_deref() == Some(who.as_str()) {
                *w = None;
            }
        }
    });
}

/// Resolves when `room_id` (or every room) got new keys; never, if the channel is gone.
/// `true` = this room's local echoes changed ("echo:<room>", L1 D08.2), `false` = keys arrived.
async fn next_keys_for(keys_rx: &mut tokio::sync::broadcast::Receiver<String>, room_id: &str) -> bool {
    loop {
        match keys_rx.recv().await {
            Ok(r) if r == room_id || r == "*" => return false,
            Ok(r) if r.strip_prefix("echo:") == Some(room_id) => return true,
            Ok(_) => {}
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => return true,
            Err(tokio::sync::broadcast::error::RecvError::Closed) => std::future::pending::<()>().await,
        }
    }
}

/// What woke the open room's live loop.
enum Wake {
    Update(Result<matrix_sdk::sync::RoomUpdate, tokio::sync::broadcast::error::RecvError>),
    Keys(bool),
}

/// Rooms whose whole key backup was pulled this session (10.5.2: once, not on every open).
fn backups_done() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static B: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> = std::sync::OnceLock::new();
    B.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
}

fn backup_download_wanted(room_id: &str, has_undecryptable: bool) -> bool {
    has_undecryptable || !backups_done().lock().map(|s| s.contains(room_id)).unwrap_or(false)
}

fn note_backup_downloaded(room_id: &str) {
    if let Ok(mut s) = backups_done().lock() {
        s.insert(room_id.to_owned());
    }
}

/// Does the room's window hold an event we still can't read?
async fn window_has_undecryptable(room_id: &str) -> bool {
    let win = timeline_window_for(room_id);
    let win = win.lock().await;
    // a deleted one keeps no content: no key can ever unlock it (2026-10-23 — it used to force a backup pull per open)
    win.events.iter().any(|v| {
        v.get("type").and_then(|t| t.as_str()) == Some("m.room.encrypted") && crate::utd::utd_row_of(v) != crate::utd::UtdRow::Deleted
    })
}

/// How a live RoomUpdate drives the open-room reload loop (see `open`).
#[derive(Clone, Copy)]
enum UpdateClass {
    /// Typing-only (or empty) update — nothing load_room_data renders changed; the
    /// dedicated typing subscription paints typing. No reload.
    Skip,
    /// Ephemeral update carrying read receipts: reload, but coalesced under
    /// RECEIPT_COALESCE so receipt churn can't storm the /messages endpoint.
    Receipt,
    /// Timeline events (incl. redactions), a `limited` gap, state/account-data
    /// changes, or a membership change: full reload after RELOAD_DEBOUNCE. `state`
    /// = the batch carried room state / account data / a membership change, so the
    /// member list + permissions must be re-derived too (9.3).
    Reload { state: bool },
}

/// 10.3: the newest event per OPEN room that is not a chat bubble (call membership, a reaction,
/// a power-level write, …), as seen in sync. `mark_read` sends the receipt there when it is newer
/// than the newest rendered row: the receipt used to stop at the last bubble, and the server kept
/// counting everything after it (an All-messages room rule counts call joins), so a room read to
/// the bottom during a call still showed dozens "missed".
fn newest_nonrow() -> &'static std::sync::Mutex<std::collections::HashMap<String, (String, u64)>> {
    static N: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, (String, u64)>>> =
        std::sync::OnceLock::new();
    N.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn note_newest_nonrow(room_id: &str, update: &matrix_sdk::sync::RoomUpdate) {
    let matrix_sdk::sync::RoomUpdate::Joined { updates, .. } = update else { return };
    let mut best: Option<(String, u64)> = None;
    for ev in &updates.timeline.events {
        let Ok(v) = ev.raw().deserialize_as::<serde_json::Value>() else { continue };
        if is_rendered_row(&v) {
            continue;
        }
        // threads (MSC3771): the main receipt may never land on a thread event — nor on a relation (an edit or reaction
        // may belong to a thread; none of them counts as unread anyway)
        if v.get("content").and_then(|c| c.get("m.relates_to")).and_then(|r| r.get("rel_type")).is_some() {
            continue;
        }
        let id = v.get("event_id").and_then(|e| e.as_str()).unwrap_or("");
        let ts = v.get("origin_server_ts").and_then(|t| t.as_u64()).unwrap_or(0);
        if id.is_empty() {
            continue;
        }
        if best.as_ref().is_none_or(|b| ts >= b.1) {
            best = Some((id.to_owned(), ts));
        }
    }
    let Some(b) = best else { return };
    note_nonrow(room_id, b.0, b.1);
}

/// newest_nonrow's key: the room for its main timeline, room \0 root for a thread's panel (12.1.17 round 2).
fn nonrow_key(room_id: &str, thread: &str) -> String {
    if thread.is_empty() { room_id.to_owned() } else { format!("{room_id}\u{0}{thread}") }
}

/// 12.1.17 round 2 (review #2/#3): what a read receipt targets. `latest` = the newest row (a server event), `nonrow` = the
/// view's newest event without a row (housekeeping, or a photo the chat hides because it is in Memories), `thread` = the
/// panel's root ("" = the main timeline). The newer of the two; a thread's root is no target (it sits in the main
/// timeline). None = nothing to read here (the main timeline's caller then takes its empty-room path). The hidden tail's
/// time is ordered by the timeline, not the clock (review r2 #6: load_room_data stores it newer than every row when the
/// timeline puts it last).
pub(crate) fn receipt_target(latest: Option<(String, u64)>, nonrow: Option<(String, u64)>, thread: &str) -> Option<String> {
    let latest = latest.filter(|(id, _)| thread.is_empty() || id != thread);
    match (latest, nonrow.filter(|(id, _)| !id.is_empty())) {
        (Some((l, lt)), Some((n, nt))) => Some(if nt > lt { n } else { l }),
        (Some((l, _)), None) => Some(l),
        // review r2 #1: a main timeline whose rows are all hidden reads its hidden tail (a "main" receipt); the empty-room
        // fallback (an unthreaded receipt on the room's newest event, which may sit in a thread) is for empty rooms only
        (None, Some((n, _))) => Some(n),
        _ => None,
    }
}

/// Remember a room's newest event that has no chat row (housekeeping; 12.1.17: a photo the chat hides because it is in
/// Memories): mark_read moves the receipt to it when it is newer than the newest row. `room_id` = nonrow_key.
fn note_nonrow(room_id: &str, id: String, ts: u64) {
    if let Ok(mut g) = newest_nonrow().lock() {
        let e = g.entry(room_id.to_owned()).or_insert((String::new(), 0));
        if ts >= e.1 {
            *e = (id, ts);
        }
    }
}

/// Classify a sync update for the live loop: does it warrant refetching the timeline?
fn classify_update(update: &matrix_sdk::sync::RoomUpdate) -> UpdateClass {
    use matrix_sdk::sync::{RoomUpdate, State};
    let RoomUpdate::Joined { updates, .. } = update else {
        return UpdateClass::Reload { state: true }; // left/invited/knocked: room changed under us
    };
    let state_events = match &updates.state {
        State::Before(e) | State::After(e) => e,
    };
    // A membership / power-level / name change can also ride the timeline as a state
    // event; treat any state-type event in the batch as "state changed".
    let timeline_state = updates.timeline.events.iter().any(|ev| {
        ev.raw().get_field::<String>("state_key").ok().flatten().is_some()
    });
    let state = !state_events.is_empty() || !updates.account_data.is_empty() || timeline_state || updates.timeline.limited;
    if updates.timeline.limited
        || !updates.timeline.events.is_empty()
        || !state_events.is_empty()
        || !updates.account_data.is_empty()
    {
        return UpdateClass::Reload { state };
    }
    // Ephemeral-only. The only ephemeral types are m.typing and m.receipt; treat
    // anything that isn't typing (incl. unparseable) as receipt-class.
    let receipts = updates
        .ephemeral
        .iter()
        .any(|ev| ev.get_field::<String>("type").ok().flatten().as_deref() != Some("m.typing"));
    if receipts {
        UpdateClass::Receipt
    } else {
        UpdateClass::Skip
    }
}

/// What one timeline load produces (9.3: a struct — the tuple had five positional fields
/// and three call sites that each spelled them out).
pub(crate) struct Loaded {
    name: String,
    json: String,
    call_active: bool,
    watch_party_on: bool,
    has_more: bool,
    game_code: String,
    pinned_json: String,
    /// widgets W1: every pinned id (the Pinned widget's "N pins")
    pinned_count: i32,
    /// widgets W3: the window's dice rolls (the Dice widget's room history)
    dice_json: String,
    /// widgets W3: the window's open polls (the Polls widget) + what a local vote / end re-tallies with
    polls_json: String,
    poll_rels: Vec<serde_json::Value>,
    poll_enders: Vec<String>,
    poll_can_redact: bool,
    own_name: String,
    own_avatar: String,
    /// (event id, url) pairs whose link preview is not cached yet — filled in the
    /// background after this snapshot is on screen (never on the load path).
    previews_wanted: Vec<(String, String)>,
    /// 12.1.17 round 2: the view's newest event when it is a photo the chat hides ("" = a row is the newest)
    read_tail: String,
}

impl Loaded {
    fn apply(self, mut o: Pin<&mut qobject::RoomController>) {
        o.as_mut().set_room_name(QString::from(&self.name));
        o.as_mut().set_messages_json(QString::from(&self.json));
        o.as_mut().set_call_active(self.call_active);
        o.as_mut().set_watch_party_on(self.watch_party_on);
        o.as_mut().set_has_more(self.has_more);
        o.as_mut().set_game_code_json(QString::from(&self.game_code));
        o.as_mut().set_pinned_json(QString::from(&self.pinned_json));
        o.as_mut().set_pinned_count(self.pinned_count);
        o.as_mut().set_dice_json(QString::from(&self.dice_json));
        o.as_mut().set_polls_json(QString::from(&self.polls_json));
        o.as_mut().set_read_tail(QString::from(&self.read_tail));
        {
            let mut r = o.as_mut().rust_mut();
            r.own_name = self.own_name;
            r.own_avatar = self.own_avatar;
            r.poll_rels = self.poll_rels;
            r.poll_enders = self.poll_enders;
            r.poll_can_redact = self.poll_can_redact;
        }
        // A30.0: the same power check (user_can_redact_event_of_other) is the message menu's "Delete" for others' messages
        o.as_mut().set_can_redact_others(self.poll_can_redact);
        if !self.previews_wanted.is_empty() {
            let gen = o.rust().open_gen;
            spawn_preview_fill(o.qt_thread(), o.room_id().to_string(), gen, self.previews_wanted);
        }
    }
}

/// 9.3: fetch the link previews a snapshot lacked and patch them into their rows as they
/// arrive (each patch bumps only that row's revision in QML). Two at a time; a preview
/// that fails is cached as "none" for a few hours, so this never loops.
fn spawn_preview_fill(
    qt: cxx_qt::CxxQtThread<qobject::RoomController>,
    room_id: String,
    gen: u64,
    wanted: Vec<(String, String)>,
) {
    crate::bridge::runtime().spawn(async move {
        use futures_util::StreamExt;
        futures_util::stream::iter(wanted)
            .for_each_concurrent(2, |(id, url)| {
                let qt = qt.clone();
                let rid = room_id.clone();
                async move {
                    let Some(p) = crate::linkpreview::fetch(&url).await else { return };
                    let _ = qt.queue(move |mut o| {
                        if !is_current(&o, gen, &rid) {
                            return;
                        }
                        let cur = o.messages_json().to_string();
                        if let Some(u) = patch_row(&cur, &id, |m| {
                            m["preview"] = p.clone();
                        }) {
                            o.as_mut().set_messages_json(QString::from(&u));
                        }
                    });
                }
            })
            .await;
    });
}

/// Load room name, recent messages (chronological JSON), live-call state, and whether
/// older history may still exist, by id. `want` = how many messages to surface.
async fn load_room(room_id: &str, want: usize, view: &str) -> Result<Loaded, String> {
    let client = crate::session::client().ok_or_else(|| "Not signed in".to_string())?;
    let rid = RoomId::parse(room_id).map_err(|e| format!("bad room id: {e}"))?;
    let room = client
        .get_room(&rid)
        .ok_or_else(|| "Room not found".to_string())?;
    load_room_data(&room, want, view).await
}

/// Joined-room member summary for the right-hand panel's member list.
struct MemberRow {
    mxid: String,
    name: String,
    avatar: String,
    power: i64,
    role: &'static str,
    membership: &'static str, // 10.1 (TH-GAP-12): "join" | "invite" | "ban"
    sort_key: i64, // power for sort; i64::MAX for room creators (Infinite power level)
    // RBAC §5.3: the visible badge — top role name + colour when the room's container has
    // roles active, else Admin|Mod|Member from the power level ("" colour); Invited/Banned
    // rows keep those words so the sections still read right.
    role_name: String,
    role_color: String,
    // 10.4 (space tags): the member-list GROUP — a hoisted top role's name + colour (roles active),
    // else Admin|Mod|Member from the power level, Invited / Banned for those rows — and the tier
    // that orders the groups: hoisted roles, then plain members, then invited, then banned. Two
    // hoisted roles with the same rank stay contiguous because the sort also keys on the name.
    group: String,
    group_color: String,
    tier: i32,
    // 10.4 (space tags): the highest CUSTOM role — what colours the name and tags it. Never a
    // template role (the design call: "not for admin or moderator or everyone, it was more of an empty
    // role for being funny"). "" when none.
    tag_name: String,
    tag_color: String,
}

/// Load the joined member list + a room summary as JSON for the right-hand panel.
/// Member/room avatars resolve through the cached `bridge::resolve_avatar`
/// (`Path::exists` short-circuit), so repeat loads are a `stat()`; always called off
/// the sync loop. `room.members()` syncs from the homeserver only on its first call
/// (`sync_members` no-ops once `are_members_synced`). Returns (members_json,
/// summary_json) — ("[]","{}") on any error.
async fn load_members(room: &Room) -> (String, String) {
    use matrix_sdk::ruma::events::room::power_levels::UserPowerLevel;
    use matrix_sdk::RoomMemberships;

    let Some(client) = crate::session::client() else {
        return ("[]".into(), "{}".into());
    };
    // 10.1 (TH-GAP-12 / TH-RP-03): invited and banned people are listed too (their own
    // sections, after the members), so an invite can be seen + withdrawn and a ban undone.
    let members = match room
        .members(RoomMemberships::JOIN | RoomMemberships::INVITE | RoomMemberships::BAN)
        .await
    {
        Ok(m) => m,
        Err(e) => {
            eprintln!("[room] members load failed: {e}");
            return ("[]".into(), "{}".into());
        }
    };

    // RBAC §5.3: ONE roles read for the whole list (the container = the parent space, or the
    // room itself), then `roles::role_badge_from` per row — never the per-call lookup in a loop.
    let roles_state = {
        let container = crate::roles::container_of_room(room).await;
        if container == room.room_id().as_str() {
            crate::roles::load_state_room(room).await
        } else {
            crate::roles::load_state(&container).await
        }
    };
    let mut rows: Vec<MemberRow> = Vec::with_capacity(members.len());
    let mut joined_count = 0usize;
    for m in &members {
        use matrix_sdk::ruma::events::room::member::MembershipState;
        let membership = match m.membership() {
            MembershipState::Invite => "invite",
            MembershipState::Ban => "ban",
            _ => "join",
        };
        if membership == "join" {
            joined_count += 1;
        }
        let (power, role, sort_key): (i64, &'static str, i64) = match (membership, m.power_level()) {
            ("invite", _) => (0, "Invited", -1),
            ("ban", _) => (0, "Banned", -2),
            (_, UserPowerLevel::Infinite) => (100, "Admin", i64::MAX),
            (_, UserPowerLevel::Int(i)) => {
                let p = i64::from(i);
                let role = if p >= 100 {
                    "Admin"
                } else if p >= 50 {
                    "Mod"
                } else {
                    "Member"
                };
                (p, role, p)
            }
            _ => (0, "Member", 0),
        };
        let avatar = match m.avatar_url() {
            Some(mxc) => crate::bridge::resolve_avatar(&client, mxc).await.unwrap_or_default(),
            None => String::new(),
        };
        let (role_name, role_color) = match membership {
            "invite" => ("Invited".to_string(), String::new()),
            "ban" => ("Banned".to_string(), String::new()),
            _ => crate::roles::role_badge_from(&roles_state, m.user_id().as_str(), power),
        };
        let (tag_name, tag_color) = match membership {
            "join" => crate::roles::tag_badge_from(&roles_state, m.user_id().as_str()),
            _ => (String::new(), String::new()),
        };
        let (group, group_color, tier): (String, String, i32) = match membership {
            "invite" => ("Invited".to_string(), String::new(), 2),
            "ban" => ("Banned".to_string(), String::new(), 3),
            _ if roles_state.active() => match roles_state.roles_of(m.user_id().as_str()).first() {
                Some(top) if top.hoist => (top.name.clone(), top.color.clone(), 0),
                _ => ("Member".to_string(), String::new(), 1),
            },
            _ => (role.to_string(), String::new(), if role == "Member" { 1 } else { 0 }),
        };
        rows.push(MemberRow {
            mxid: m.user_id().to_string(),
            name: m.name().to_string(),
            avatar,
            power,
            role,
            membership,
            sort_key,
            role_name,
            role_color,
            group,
            group_color,
            tier,
            tag_name,
            tag_color,
        });
    }
    // Sort by tier (hoisted roles, members, invited, banned), power (desc), group name (so two
    // hoisted roles of equal rank do not interleave), then display name (case-insensitive asc).
    rows.sort_by(|a, b| {
        a.tier
            .cmp(&b.tier)
            .then_with(|| b.sort_key.cmp(&a.sort_key))
            .then_with(|| a.group.cmp(&b.group))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    let members_val: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "mxid": r.mxid,
                "name": r.name,
                "avatar": r.avatar,
                "power": r.power,
                "role": r.role,
                "membership": r.membership,
                "roleName": r.role_name,
                "roleColor": r.role_color,
                "group": r.group,
                "groupColor": r.group_color,
                "tagName": r.tag_name,
                "tagColor": r.tag_color,
            })
        })
        .collect();
    let members_json = serde_json::to_string(&members_val).unwrap_or_else(|_| "[]".into());

    let room_avatar = match room.avatar_url() {
        Some(mxc) => crate::bridge::resolve_avatar(&client, &mxc).await.unwrap_or_default(),
        None => String::new(),
    };
    // DM/unnamed rooms have no m.room.name; the SDK's display_name() resolves those to
    // the other member's name (mirrors nav.rs / bridge.rs), so the info panel header
    // matches the room list instead of showing a raw "!roomid:server".
    let display = match room.display_name().await {
        Ok(d) => d.to_string(),
        Err(_) => room
            .name()
            .or_else(|| room.canonical_alias().map(|a| a.to_string()))
            .unwrap_or_else(|| room.room_id().to_string()),
    };
    let summary = serde_json::json!({
        "name": display,
        "topic": room.topic().unwrap_or_default(),
        "alias": room.canonical_alias().map(|a| a.to_string()).unwrap_or_default(),
        "encrypted": room.encryption_state().is_encrypted(),
        "memberCount": joined_count,
        "avatar": room_avatar,
    });
    let summary_json = serde_json::to_string(&summary).unwrap_or_else(|_| "{}".into());

    // Cache for instant paint on the next open (mirrors the timeline cache). Only when
    // we actually got members, so a transient empty load never clobbers a good cache.
    if !members.is_empty() {
        write_members_cache(room.room_id().as_str(), &members_json, &summary_json);
    }

    (members_json, summary_json)
}

/// `load_members` for the refresh() path, which only has a room id in scope.
async fn load_members_by_id(room_id: &str) -> (String, String) {
    match room_by_id(room_id).await {
        Ok(room) => load_members(&room).await,
        Err(_) => ("[]".into(), "{}".into()),
    }
}

/// Per-room raw-event window kept between reloads so a live update costs ONE page
/// (backward from the tip, stopping at the newest already-known event id) instead of
/// re-paginating the whole window — a shipped field log showed 24 sequential
/// /messages round-trips PER NEW MESSAGE in the call-heavy group room ("scanned 2400
/// events / 24 pages -> 34 messages"), which is exactly the friends' "chat is slow /
/// participants are slow" (the member refresh queues behind the same fetch).
/// `events` holds the post-decryption raw JSON, newest-first — the derive pass below
/// rebuilds reactions/edits/game-code/receipts from it byte-identically to the old
/// inline walk. Redactions are STAMPED onto their cached target (content emptied +
/// unsigned.redacted_because set, matching what the server returns on a refetch).
struct TimelineWindow {
    events: Vec<serde_json::Value>,
    ids: std::collections::HashSet<String>,
    from_token: Option<String>,
    /// Rendered-row count (non-edit m.room.message) — the `want` currency.
    msg_rows: usize,
    /// 10.5.2: the keys epoch (`keys_epoch`) its events were last decrypted against.
    decrypt_epoch: u64,
}

impl TimelineWindow {
    fn new() -> Self {
        Self {
            events: Vec::new(),
            ids: std::collections::HashSet::new(),
            from_token: None,
            msg_rows: 0,
            decrypt_epoch: 0,
        }
    }
}

/// True for events that render as their own chat bubble (what `want` counts).
fn is_rendered_row(v: &serde_json::Value) -> bool {
    // threads (review #3): a thread reply is no row of the main timeline, so it never fills the main view's quota (a
    // busy thread would otherwise page the room's own messages out of the window)
    if !tinhat_widgets::threads::thread_of(v.get("content").unwrap_or(&serde_json::Value::Null)).is_empty() {
        return false;
    }
    let ty = v.get("type").and_then(|t| t.as_str());
    // An event still sitting as m.room.encrypted after the SDK's decryption pass is one
    // we hold no key for. It is a real message we simply cannot read, so it occupies a
    // row. Counting it here is also what lets the history loop terminate: rooms
    // whose backlog we cannot decrypt used to make it paginate the entire room.
    if ty == Some("m.room.encrypted") {
        // 2026-10-23: except an unreadable edit / poll vote / reaction (utd.rs utd_row: not a row)
        return crate::utd::utd_row_of(v) != crate::utd::UtdRow::Hidden;
    }
    if ty != Some("m.room.message") {
        return false;
    }
    v.get("content")
        .and_then(|c| c.get("m.relates_to"))
        .and_then(|r| r.get("rel_type"))
        .and_then(|r| r.as_str())
        != Some("m.replace")
}

fn timeline_windows(
) -> &'static std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<TimelineWindow>>>> {
    static W: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<TimelineWindow>>>>,
    > = std::sync::OnceLock::new();
    W.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn timeline_window_for(room_id: &str) -> std::sync::Arc<tokio::sync::Mutex<TimelineWindow>> {
    timeline_windows()
        .lock()
        .unwrap()
        .entry(room_id.to_string())
        .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(TimelineWindow::new())))
        .clone()
}

/// Stamp a redaction onto its cached target: empty the content and set
/// unsigned.redacted_because — the same shape a fresh /messages fetch returns for a
/// redacted event, so the derive pass needs no special case. Covers messages
/// (renders as "redacted"), reactions (aggregation skips: no m.relates_to left)
/// and edits (fold skips: no rel_type left).
fn stamp_redaction(win: &mut TimelineWindow, redaction: &serde_json::Value) {
    let Some(target) = redaction
        .get("redacts")
        .and_then(|r| r.as_str())
        .or_else(|| redaction.get("content").and_then(|c| c.get("redacts")).and_then(|r| r.as_str()))
    else {
        return;
    };
    // QA W1: an older pin fetched before it was deleted is fetched again (and then reads as deleted)
    if let Ok(mut m) = pinned_rows().lock() {
        m.remove(target);
    }
    for v in win.events.iter_mut() {
        if v.get("event_id").and_then(|e| e.as_str()) == Some(target) {
            stamp_redacted(v, redaction);
            break;
        }
    }
}

/// One event redacted in place. threads: a thread reply keeps its thread relation, so a deleted reply stays in its
/// thread (the main timeline never shows it as a stray "deleted" row).
fn stamp_redacted(v: &mut serde_json::Value, redaction: &serde_json::Value) {
    let rel = v
        .get("content")
        .and_then(|c| c.get("m.relates_to"))
        .filter(|r| !tinhat_widgets::threads::thread_of(&serde_json::json!({ "m.relates_to": r })).is_empty())
        .cloned();
    v["content"] = match rel {
        Some(r) => serde_json::json!({ "m.relates_to": r }),
        None => serde_json::json!({}),
    };
    v["unsigned"]["redacted_because"] = redaction.clone();
}

/// The event types the window pages through (server-side filter).
fn window_filter_types() -> Vec<String> {
    vec![
        "m.room.message".to_owned(),
        "m.room.encrypted".to_owned(),
        "m.reaction".to_owned(),
        // Window mode addition: redactions must reach us to be stamped onto their
        // cached targets (a full refetch used to pick up the server-side effect).
        "m.room.redaction".to_owned(),
        // Game join-code: a custom timeline event any member can post (PL 0). In
        // E2EE rooms it rides inside m.room.encrypted + decrypts to this type.
        "dev.tinhat.game.code".to_owned(),
    ]
}

/// Hard ceiling on events examined in ONE pass. max_pages alone was not a bound on
/// work: 80 pages x 100 events is 8,000 events deserialized and walked while the user
/// waits. One busy room hit 2,400 in a single load for one row. When this
/// trips we return what we have with more=true, and the next pull resumes from the
/// stored token — responsive beats complete.
const SCAN_BUDGET: usize = 900;

/// One backward `/messages` page as the window sync sees it: event JSON (post-decryption where
/// we could), newest first, and the continuation token (None = start of the room).
struct WindowPage {
    chunk: Vec<serde_json::Value>,
    end: Option<String>,
}

/// What one window pass did (the log line reads it; so do the tests).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct WindowPass {
    pages: u32,
    scanned: usize,
    hit_frontier: bool,
    /// A WARM window that did not reach its frontier within one warm pass and was rebuilt.
    gap: bool,
}

/// Bring the room's window up to date and at least `want` rows deep (budget permitting).
/// `fetch(from, limit)` returns the next backward page (from the tip when `from` is None).
///
/// 10.5.2 (timeline): going back into a room after a gap bigger than one warm pass (24 pages x 30
/// events) REPLACED the window with just the newest events — and in a call-heavy encrypted room
/// ~95% of those are call media-key events, not messages (one busy room: 849 of its newest 900).
/// The tail walk had no budget left, so the room came back with a handful of rows or none at all
/// ("No messages"), and only a scroll gesture could fetch more. A gap — or a pass that found no
/// rows — now gets a fresh tail budget, so the window is refilled to `want` rows in the same pass.
async fn sync_window<F, Fut>(win: &mut TimelineWindow, want: usize, mut fetch: F) -> Result<WindowPass, String>
where
    F: FnMut(Option<String>, u16) -> Fut,
    Fut: std::future::Future<Output = Result<WindowPage, String>>,
{
    let max_pages: u32 = ((want as u32 / 4) + 12).min(80);
    let was_warm = !win.events.is_empty();
    let mut pages = 0u32;
    let mut scanned = 0usize;
    let mut fresh: Vec<serde_json::Value> = Vec::new();
    let mut fresh_rows = 0usize;
    let mut redactions: Vec<serde_json::Value> = Vec::new();
    let mut hit_frontier = false;
    let mut from: Option<String> = None;
    let mut tip_end: Option<String>;
    loop {
        // 9.3: a WARM window only needs the handful of events since the frontier, which
        // is almost always inside the first few — a 100-event page per incoming message
        // was 50-100 KB of JSON deserialised and discarded on every line of chat.
        let page: u16 = if win.events.is_empty() { 100 } else { 30 };
        let resp = fetch(from.clone(), page).await?;
        if resp.chunk.is_empty() {
            tip_end = resp.end;
            break;
        }
        scanned += resp.chunk.len();
        for v in resp.chunk {
            let id = v.get("event_id").and_then(|e| e.as_str()).unwrap_or("").to_string();
            if !id.is_empty() && win.ids.contains(&id) {
                hit_frontier = true;
                break;
            }
            if v.get("type").and_then(|t| t.as_str()) == Some("m.room.redaction") {
                if !id.is_empty() {
                    win.ids.insert(id); // frontier-capable, but never a window row
                }
                redactions.push(v);
                continue;
            }
            if id.is_empty() {
                continue; // spec-violating id-less event: unkeyable, skip
            }
            if is_rendered_row(&v) {
                fresh_rows += 1;
            }
            fresh.push(v);
        }
        tip_end = resp.end;
        pages += 1;
        // SCAN_BUDGET applies HERE too. This loop stops at the frontier, so a
        // stale window over a big room walked thousands of events before the budgeted
        // loop below ever ran — 0.6.3's log still showed `24 pages / 2400 scanned`
        // alongside a correctly-capped `9 pages / 900`. Falling out early lands in the
        // cold-rebuild branch, which already handles "we stopped before the frontier".
        if hit_frontier || tip_end.is_none() || pages >= max_pages || scanned >= SCAN_BUDGET {
            break;
        }
        if win.events.is_empty() && fresh_rows >= want {
            break;
        }
        from = tip_end.clone();
    }
    let gap = was_warm && !hit_frontier;
    if hit_frontier {
        for v in fresh.iter() {
            if let Some(id) = v.get("event_id").and_then(|e| e.as_str()) {
                win.ids.insert(id.to_string());
            }
        }
        win.msg_rows += fresh_rows;
        let mut merged = fresh;
        merged.append(&mut win.events);
        win.events = merged;
    } else {
        // Cold load, or the gap since the last reload exceeded the page cap.
        // Logged because a WARM room taking this branch REPLACES the window, which makes
        // the QML timeline rebuild every delegate under the reader — the leading suspect
        // for the reported "glitchy as it reformats the content it has".
        if gap {
            eprintln!(
                "[room] WINDOW REBUILD (frontier not reached after {pages} pages / {scanned} events) — {} old events discarded, {fresh_rows} rows in the new ones",
                win.events.len()
            );
        }
        win.ids.clear();
        for v in fresh.iter() {
            if let Some(id) = v.get("event_id").and_then(|e| e.as_str()) {
                win.ids.insert(id.to_string());
            }
        }
        win.msg_rows = fresh_rows;
        win.events = fresh;
        win.from_token = tip_end;
    }
    for r in std::mem::take(&mut redactions) {
        stamp_redaction(win, &r);
    }
    // The tail walk (older history). A gap starts it with a fresh budget (see the doc comment), and
    // so does a pass that found no rows at all — a COLD open whose newest 900 events are all call
    // keys painted "No messages" over the room's snapshot. Everything else shares the budget the
    // walk above used, as before. Either way at most 2 x SCAN_BUDGET per pass, and every pass moves
    // the window further back, so a later one never re-walks the same events.
    let fresh_budget = gap || win.msg_rows == 0;
    let (mut tail_pages, mut tail_scanned) = if fresh_budget { (0u32, 0usize) } else { (pages, scanned) };
    while win.msg_rows < want && tail_pages < max_pages && tail_scanned < SCAN_BUDGET {
        let Some(tok) = win.from_token.clone() else { break };
        let resp = fetch(Some(tok), 100).await?;
        win.from_token = resp.end.clone();
        tail_pages += 1;
        pages += 1;
        if resp.chunk.is_empty() {
            if win.from_token.is_none() {
                break;
            }
            continue;
        }
        tail_scanned += resp.chunk.len();
        scanned += resp.chunk.len();
        for v in resp.chunk {
            let id = v.get("event_id").and_then(|e| e.as_str()).unwrap_or("").to_string();
            if id.is_empty() || win.ids.contains(&id) {
                continue;
            }
            if v.get("type").and_then(|t| t.as_str()) == Some("m.room.redaction") {
                win.ids.insert(id);
                continue; // older than its target by definition; server already served the effect
            }
            win.ids.insert(id);
            if is_rendered_row(&v) {
                win.msg_rows += 1;
            }
            win.events.push(v); // append = older (window is newest-first)
        }
        if win.from_token.is_none() {
            break;
        }
    }
    Ok(WindowPass { pages, scanned, hit_frontier, gap })
}

/// 10.5.2: most decryption attempts in one re-decrypt pass (local crypto-store lookups only). One
/// attempt per megolm SESSION that still fails, so this is rarely reached.
const REDECRYPT_MAX: usize = 400;

/// Re-try the window's undecryptable events against the keys we hold NOW. The window keeps raw
/// events between reloads and only ever fetches NEW ones, so a message whose key arrived after it
/// (a late key share, the backup download) kept saying "Can't read this message" for the rest of
/// the session — every time you went back into the room. A session that still fails is not tried
/// again in the same pass. Returns (how many became readable, whether every event was looked at).
async fn redecrypt_window(room: &Room, win: &mut TimelineWindow) -> (usize, bool) {
    use matrix_sdk::deserialized_responses::TimelineEventKind;
    use matrix_sdk::ruma::events::room::encrypted::OriginalSyncRoomEncryptedEvent;
    use matrix_sdk::ruma::serde::Raw;
    let mut fixed = 0usize;
    let mut tried = 0usize;
    let mut complete = true;
    let mut dead: std::collections::HashSet<String> = std::collections::HashSet::new();
    for v in win.events.iter_mut() {
        if v.get("type").and_then(|t| t.as_str()) != Some("m.room.encrypted") {
            continue;
        }
        let session = v
            .get("content")
            .and_then(|c| c.get("session_id"))
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string();
        if !session.is_empty() && dead.contains(&session) {
            continue;
        }
        if tried >= REDECRYPT_MAX {
            complete = false;
            break;
        }
        tried += 1;
        let Ok(json) = serde_json::value::to_raw_value(&*v) else { continue };
        let raw: Raw<OriginalSyncRoomEncryptedEvent> = Raw::from_json(json);
        let readable = match room.decrypt_event(&raw, None).await {
            Ok(ev) if matches!(ev.kind, TimelineEventKind::Decrypted(_)) => {
                ev.raw().deserialize_as::<serde_json::Value>().ok().map(|mut v| {
                    crate::moved::normalize(&mut v);
                    v
                })
            }
            Ok(ev) => {
                // selfhost/firstrun §3: still unreadable — keep matrix-sdk's latest reason for the row's words
                if let TimelineEventKind::UnableToDecrypt { utd_info, .. } = &ev.kind {
                    crate::utd::annotate(v, &utd_info.reason);
                }
                None
            }
            _ => None,
        };
        match readable {
            Some(dv) => {
                *v = dv;
                fixed += 1;
            }
            None if !session.is_empty() => {
                dead.insert(session);
            }
            None => {}
        }
    }
    if fixed > 0 {
        win.msg_rows = win.events.iter().filter(|v| is_rendered_row(v)).count();
    }
    (fixed, complete)
}

/// Same, given a resolved Room (used by the live subscription). `want` = window size.
async fn load_room_data(room: &Room, want: usize, view: &str) -> Result<Loaded, String> {
    // threads (round 2 #12): the session this load belongs to, read before anything awaits
    let session_gen = crate::session::generation();
    // Use the SDK's computed display name so DMs (no m.room.name) show the partner's
    // name in the header + composer placeholder, not the raw "!roomid:server". Same
    // fallback chain as nav.rs (display_name -> name -> canonical_alias -> room_id).
    let name = match room.display_name().await {
        Ok(d) => d.to_string(),
        Err(_) => room
            .name()
            .or_else(|| room.canonical_alias().map(|a| a.to_string()))
            .unwrap_or_else(|| room.room_id().to_string()),
    };
    let call_active = crate::rtc::call_going_on(room).await;

    // ---- Window sync (the only network part) ----
    // Warm path: one backward page from the tip, stopping at the newest KNOWN event
    // id, splice the new events in. Cold path / gap beyond the page cap: rebuild the
    // window with the old full-walk semantics. `want` deeper than the window (a
    // load_older) extends the tail from the stored continuation token. (10.5.2: the
    // walk itself is `sync_window`, driven here by real /messages pages.)
    let rid_key = room.room_id().to_string();
    // 12.1.17 (docs/MEMORIES-PHOTOS.md "In the chat"): what the chat hides — a photo its sender's own Memories list
    // names (memphotos::hidden_in_chat, fail closed); read before the window is locked (state-store reads, review)
    let mem_idx = crate::memories_photos::chat_index(room).await;
    let win_arc = timeline_window_for(&rid_key);
    let mut win = win_arc.lock().await;
    // 10.5.2: keys that arrived since this window last decrypted → re-try its unreadable events, BEFORE
    // the network part (a failing /messages must not keep them unreadable); what this pass fetches
    // is decrypted with today's keys anyway.
    let epoch_now = keys_epoch(&rid_key);
    if win.events.is_empty() {
        win.decrypt_epoch = epoch_now;
    } else if epoch_now != win.decrypt_epoch {
        let (fixed, complete) = redecrypt_window(room, &mut win).await;
        if complete {
            win.decrypt_epoch = epoch_now;
        }
        if fixed > 0 {
            eprintln!("[room] {} re-decrypted {fixed} event(s) whose keys arrived after them", room.room_id());
        }
    }
    let fetch = move |from: Option<String>, limit: u16| async move {
        let mut opts = MessagesOptions::backward();
        opts.from = from;
        opts.limit = UInt::from(limit);
        opts.filter.types = Some(window_filter_types());
        let resp = room.messages(opts).await.map_err(|e| e.to_string())?;
        let chunk = resp
            .chunk
            .iter()
            .filter_map(|ev| {
                let mut v = ev.raw().deserialize_as::<serde_json::Value>().ok()?;
                crate::moved::normalize(&mut v); // a moved server's events: today's ids (moved.rs)
                // selfhost/firstrun §3: why it couldn't be read (the row's words come from it)
                if let matrix_sdk::deserialized_responses::TimelineEventKind::UnableToDecrypt { utd_info, .. } = &ev.kind {
                    crate::utd::annotate(&mut v, &utd_info.reason);
                }
                Some(v)
            })
            .collect();
        Ok::<WindowPage, String>(WindowPage { chunk, end: resp.end })
    };
    let pass = sync_window(&mut win, want, fetch).await?;
    let mut has_more = win.from_token.is_some();
    // threads (docs/THREADS.md): a thread's panel rows its thread — the window's events in it + the server's
    // /relations pages of it (thread_window_events) + the root; the main view rows the window minus thread replies
    let thread_events: Vec<serde_json::Value> = if view.is_empty() {
        Vec::new()
    } else {
        // boxed: a deep future (rustc's query depth limit in the open() task)
        let (evs, more) = Box::pin(thread_window_events(room, view, want, &win.events, session_gen)).await;
        has_more = more;
        evs
    };
    let src: &Vec<serde_json::Value> = if view.is_empty() { &win.events } else { &thread_events };
    let plan = crate::threads_rows::thread_plan(src, if view.is_empty() { None } else { Some(view) });
    eprintln!(
        "[room] {} window: {} pages / {} scanned this pass -> {} events / {} rows (want={want}, more={has_more}, frontier={}){}",
        room.room_id(),
        pass.pages,
        pass.scanned,
        win.events.len(),
        win.msg_rows,
        pass.hit_frontier,
        if pass.gap { " [gap refilled]" } else { "" }
    );

    // ---- Derive the view from the window (local, no network) ----
    // selfhost/firstrun §3: this device's facts for the words of unreadable messages, read once (store reads)
    let utd_ctx = if win.events.iter().any(|v| v.get("type").and_then(|t| t.as_str()) == Some("m.room.encrypted")) {
        crate::utd::context(&room.client()).await
    } else {
        crate::utd::Context::default()
    };
    let mut msgs: Vec<serde_json::Value> = Vec::new();
    // Per-sender display-name cache (resolve once per distinct sender, no network).
    let mut names: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    // Reactions: target_event_id -> its reaction events, NEWEST first as scanned (reversed and grouped into chips by
    // crate::reactions::group_chips below: stable order, one count per sender, ❤ = ❤️ — QOL A07.0/A07.1/A07.2).
    let mut reactions: std::collections::HashMap<String, Vec<crate::reactions::ReactIn>> = std::collections::HashMap::new();
    // Edits (m.replace): target_event_id -> (new_body, new_formatted). Paginating
    // backward, the first edit seen for a target is the most recent, so keep that.
    let mut edits: std::collections::HashMap<String, (String, String)> = std::collections::HashMap::new();
    // Latest game join-code in the scanned window (by origin_server_ts). "" = none/cleared.
    let mut game_code = String::new();
    let mut game_code_ts = 0u64;
    // Deferred m.image resolves: (msgs index, content, event_id, mimetype) — fetched
    // with bounded concurrency after pagination so downloads don't serialize the load.
    let mut pending_media: Vec<(usize, serde_json::Value, String, Option<String>)> = Vec::new();
    let my_id = room.own_user_id().to_string();
    // 9.3: my display name (mention highlight by name, echo rows) + per-sender avatars
    // (profile pictures on bubbles — the rows never carried one) + the pinned set.
    let my_name = resolve_member_name(room, &my_id).await;
    // threads: my receipts that count for the summaries' unread dots (the main view's roots: replies seen or a bundle)
    let thread_receipts = if view.is_empty() {
        let mut roots: Vec<String> = plan.seen.keys().cloned().collect();
        for v in src.iter() {
            if tinhat_widgets::threads::bundle_of(v).is_some() {
                roots.push(crate::threads_rows::ev_id(v).to_string());
            }
        }
        roots.sort();
        roots.dedup();
        if roots.is_empty() { Vec::new() } else { Box::pin(crate::receipts::my_thread_receipts(room, &roots)).await }
    } else {
        Vec::new()
    };
    let my_name_lc = my_name.to_lowercase();
    let mut avatars: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    // widgets W1: the pins in the shared order (newest pin first, each once — tinhat_widgets::recent::pin_order, the
    // web's pinOrder), so the older ones fetched below are the NEWEST 20 and each row knows its place (pinIndex).
    let pin_order: Vec<String> = tinhat_widgets::recent::pin_order(&serde_json::json!({
        "pinned": room.pinned_event_ids().unwrap_or_default().iter().map(|e| e.to_string()).collect::<Vec<_>>()
    }));
    let pinned_ids: std::collections::HashSet<String> = pin_order.iter().cloned().collect();
    // widgets W3: the window's poll responses and ends (the cards are tallied after the loop)
    let mut poll_rels: Vec<serde_json::Value> = Vec::new();
    // 12.1.17: the photos the chat hides (mem_idx): id → (sender, its name) for the reply quotes, the newest of them
    // (the read receipt still moves past it) and a stub each (an echo of it is delivered).
    let mut in_memories: std::collections::HashMap<String, (String, String)> = std::collections::HashMap::new();
    let mut hidden_newest: Option<(String, u64)> = None;
    // review r2 #6: the newest hidden photo is newer than every row (timeline order); each hidden photo → how many rows
    // the window had met before it (= rows newer than it: the row before it is found by position, never by clock)
    let mut hidden_is_tail = false;
    let mut rows_newer: Vec<(String, usize)> = Vec::new();
    let mut hidden_stubs: Vec<serde_json::Value> = Vec::new();
    {
        for (vi, v) in src.iter().enumerate() {
            // threads: the main timeline leaves thread replies (and what relates to them) out; a panel its thread's
            if !plan.in_view[vi] {
                continue;
            }
            let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("?");
            // Reactions (m.annotation): aggregate per target event + emoji key.
            if ty == "m.reaction" {
                if let Some(rel) = v.get("content").and_then(|c| c.get("m.relates_to")) {
                    let target = rel.get("event_id").and_then(|e| e.as_str()).unwrap_or("");
                    let key = rel.get("key").and_then(|k| k.as_str()).unwrap_or("");
                    let rsender = v.get("sender").and_then(|s| s.as_str()).unwrap_or("");
                    let rid = v.get("event_id").and_then(|e| e.as_str()).unwrap_or("");
                    if !target.is_empty() && !key.is_empty() {
                        let who = if let Some(n) = names.get(rsender) {
                            n.clone()
                        } else {
                            let n = resolve_member_name(room, rsender).await;
                            names.insert(rsender.to_string(), n.clone());
                            n
                        };
                        // 10.1 (TH-MSG-10): one person counts once — by sender since QOL A07.1 (group_chips)
                        reactions.entry(target.to_string()).or_default().push(crate::reactions::ReactIn {
                            key: key.to_string(),
                            sender: rsender.to_string(),
                            event_id: rid.to_string(),
                            name: who,
                        });
                    }
                }
                continue;
            }
            // Game join-code (custom timeline event): track the most-recent one (by
            // origin_server_ts) to surface in the dedicated join-code bar. Any member can
            // post it; an empty `code` means "cleared". Never rendered as a chat bubble.
            if ty == "dev.tinhat.game.code" {
                let ts = v.get("origin_server_ts").and_then(|t| t.as_u64()).unwrap_or(0);
                if ts >= game_code_ts {
                    let c = v.get("content");
                    let code = c.and_then(|c| c.get("code")).and_then(|s| s.as_str()).unwrap_or("");
                    let game = c.and_then(|c| c.get("game")).and_then(|s| s.as_str()).unwrap_or("");
                    let sender = v.get("sender").and_then(|s| s.as_str()).unwrap_or("");
                    game_code_ts = ts;
                    game_code = if code.trim().is_empty() {
                        String::new()
                    } else {
                        serde_json::json!({ "game": game, "code": code, "sender": sender }).to_string()
                    };
                }
                continue;
            }
            // 2026-10-23 (utd.rs utd_row = the web's utdRow): an unreadable edit / poll vote / reaction is no row of its
            // own; an unreadable DELETED message is a deleted message (the message branch below shows it so, like the web).
            let utd_row = if ty == "m.room.encrypted" { Some(crate::utd::utd_row_of(v)) } else { None };
            if utd_row == Some(crate::utd::UtdRow::Hidden) {
                continue;
            }
            let ty = if utd_row == Some(crate::utd::UtdRow::Deleted) { "m.room.message" } else { ty };
            // Undecryptable: show that something is there rather than a silent gap.
            if utd_row == Some(crate::utd::UtdRow::Row) {
                let sender = v.get("sender").and_then(|s| s.as_str()).unwrap_or("").to_string();
                let eid = v.get("event_id").and_then(|e| e.as_str()).unwrap_or("").to_string();
                let ts = v.get("origin_server_ts").and_then(|t| t.as_u64()).unwrap_or(0);
                // Same cached resolve the message branch uses (no network per row).
                let name = if let Some(n) = names.get(&sender) {
                    n.clone()
                } else {
                    let n = resolve_member_name(room, &sender).await;
                    names.insert(sender.clone(), n.clone());
                    n
                };
                let avatar = sender_avatar(room, &sender, &mut avatars).await;
                // selfhost/firstrun §3: why it can't be read, in the web's words (utd.rs)
                let cause = crate::utd::cause_of(v, &utd_ctx);
                msgs.push(serde_json::json!({
                    "id": eid,
                    "sender": sender,
                    "senderName": name,
                    "avatar": avatar,
                    "body": cause.text(),
                    "utdCause": cause.key(),
                    "msgtype": "m.text",
                    "undecryptable": true,
                    "ts": ts,
                }));
                continue;
            }
            // widgets W3 (WIDGETS.md §17): polls — a response or an end feeds its poll's tally (every card is built after the
            // loop, from the whole window); a start is a row of its own (the poll card), "Poll: <question>" as its text
            if tinhat_widgets::polls::RESPONSE_TYPES.contains(&ty) || tinhat_widgets::polls::END_TYPES.contains(&ty) {
                if v.get("unsigned").and_then(|u| u.get("redacted_because")).is_none() {
                    poll_rels.push(serde_json::json!({
                        "id": v.get("event_id").cloned().unwrap_or_default(), "type": ty,
                        "sender": v.get("sender").cloned().unwrap_or_default(),
                        "ts": v.get("origin_server_ts").cloned().unwrap_or_default(),
                        "content": v.get("content").cloned().unwrap_or_default(),
                    }));
                }
                continue;
            }
            if tinhat_widgets::polls::START_TYPES.contains(&ty) {
                let sender = v.get("sender").and_then(|s| s.as_str()).unwrap_or("").to_string();
                let id = v.get("event_id").and_then(|e| e.as_str()).unwrap_or("").to_string();
                let ts = v.get("origin_server_ts").and_then(|t| t.as_u64()).unwrap_or(0);
                let redacted = v.get("unsigned").and_then(|u| u.get("redacted_because")).is_some();
                let content = v.get("content").cloned().unwrap_or_default();
                let question = if redacted { None } else { tinhat_widgets::polls::read_start(ty, &content).map(|p| p.question) };
                if !redacted && question.is_none() {
                    continue; // a start this app can't read shows nothing (as in Element)
                }
                let sender_name = if let Some(n) = names.get(&sender) {
                    n.clone()
                } else {
                    let n = resolve_member_name(room, &sender).await;
                    names.insert(sender.clone(), n.clone());
                    n
                };
                let avatar = sender_avatar(room, &sender, &mut avatars).await;
                let server_txn = v.get("unsigned").and_then(|u| u.get("transaction_id")).and_then(|t| t.as_str()).unwrap_or("");
                let body = question.map_or(String::new(), |q| format!("Poll: {q}"));
                msgs.push(serde_json::json!({
                    "sender": sender, "senderName": sender_name, "body": body, "ts": ts, "id": id,
                    "avatar": avatar, "mentionsMe": false, "pinned": !redacted && pinned_ids.contains(&id),
                    "serverTxn": server_txn,
                    "msgtype": POLL_MSGTYPE, "formatted": "", "media": "",
                    "mxc": "", "mimetype": "", "w": 0, "h": 0, "durationMs": 0, "size": 0,
                    "replyTo": "", "redacted": redacted, "embeds": [], "onlyEmbed": false,
                    "calContainer": "", "calEvent": "", "themeShare": serde_json::Value::Null, "pinSnippet": serde_json::Value::Null,
                    "dice": serde_json::Value::Null, "poll": serde_json::Value::Null,
                    "pollType": ty, "pollContent": if redacted { serde_json::Value::Null } else { content },
                }));
                continue;
            }
            if ty != "m.room.message" {
                continue;
            }
            // Edits (m.replace): fold the new content into the target later; don't
            // render the edit event as its own bubble.
            if let Some(rel) = v.get("content").and_then(|c| c.get("m.relates_to")) {
                if rel.get("rel_type").and_then(|r| r.as_str()) == Some("m.replace") {
                    if let Some(target) = rel.get("event_id").and_then(|e| e.as_str()) {
                        let nc = v.get("content").and_then(|c| c.get("m.new_content"));
                        let nb = nc
                            .and_then(|c| c.get("body"))
                            .and_then(|b| b.as_str())
                            .unwrap_or("")
                            .to_string();
                        let nf = nc
                            .and_then(|c| {
                                if c.get("format").and_then(|f| f.as_str()) == Some("org.matrix.custom.html") {
                                    c.get("formatted_body").and_then(|b| b.as_str())
                                } else {
                                    None
                                }
                            })
                            .unwrap_or("")
                            .to_string();
                        edits.entry(target.to_string()).or_insert((nb, nf));
                    }
                    continue;
                }
            }
            let sender = v.get("sender").and_then(|s| s.as_str()).unwrap_or("");
            let content = v.get("content");
            let body = content
                .and_then(|c| c.get("body"))
                .and_then(|b| b.as_str())
                .unwrap_or("");
            let ts = v.get("origin_server_ts").and_then(|t| t.as_u64()).unwrap_or(0);
            let id = v.get("event_id").and_then(|e| e.as_str()).unwrap_or("");
            // msgtype drives rendering (text / emote / notice / image / file / …).
            let msgtype = content
                .and_then(|c| c.get("msgtype"))
                .and_then(|m| m.as_str())
                .unwrap_or("m.text");
            // 12.1.17: a photo in its sender's own Memories leaves the chat (it still heads its own comments' panel)
            if id != view && crate::memories_photos::hidden_event(mem_idx.as_ref(), v) {
                let sn = if let Some(n) = names.get(sender) {
                    n.clone()
                } else {
                    let n = resolve_member_name(room, sender).await;
                    names.insert(sender.to_string(), n.clone());
                    n
                };
                in_memories.insert(id.to_string(), (sender.to_string(), sn));
                // review r2 #6: the window runs newest first, so the first hidden photo met is the newest, and it is the
                // view's tail when no row has been met yet (timeline order, never equal clocks)
                if hidden_newest.is_none() {
                    hidden_newest = Some((id.to_string(), ts));
                    hidden_is_tail = msgs.is_empty();
                }
                rows_newer.push((id.to_string(), msgs.len()));
                let txn = v.pointer("/unsigned/transaction_id").and_then(|t| t.as_str()).unwrap_or("");
                hidden_stubs.push(serde_json::json!({"id": id, "sender": sender, "msgtype": msgtype, "body": body, "ts": ts, "serverTxn": txn}));
                continue;
            }
            // HTML formatted body (markdown rendered) — shown as rich text when present.
            let formatted_raw = content
                .and_then(|c| {
                    if c.get("format").and_then(|f| f.as_str()) == Some("org.matrix.custom.html") {
                        c.get("formatted_body").and_then(|b| b.as_str())
                    } else {
                        None
                    }
                })
                .unwrap_or("");
            // security review 2026-10-26 (H2): another member's HTML is sanitised before QML's rich text sees it
            // (richtext.rs); the spoilers are hidden after (10.1 TH-MSG-05)
            let formatted_owned = crate::richtext::message_html(formatted_raw);
            let formatted = formatted_owned.as_str();
            // Media source: plaintext `url`, or encrypted `file.url` (mxc://…).
            let media_url = content
                .and_then(|c| {
                    c.get("url")
                        .and_then(|u| u.as_str())
                        .or_else(|| c.get("file").and_then(|f| f.get("url")).and_then(|u| u.as_str()))
                })
                .unwrap_or("");
            let info = content.and_then(|c| c.get("info"));
            let mimetype = info
                .and_then(|i| i.get("mimetype"))
                .and_then(|m| m.as_str())
                .unwrap_or("");
            let w = info.and_then(|i| i.get("w")).and_then(|x| x.as_u64()).unwrap_or(0);
            let h = info.and_then(|i| i.get("h")).and_then(|x| x.as_u64()).unwrap_or(0);
            let duration_ms =
                info.and_then(|i| i.get("duration")).and_then(|x| x.as_u64()).unwrap_or(0);
            let size_bytes = info.and_then(|i| i.get("size")).and_then(|x| x.as_u64()).unwrap_or(0);
            // Friendly display name, resolved once per sender (cached, no network).
            let sender_name = if let Some(n) = names.get(sender) {
                n.clone()
            } else {
                let n = resolve_member_name(room, sender).await;
                names.insert(sender.to_string(), n.clone());
                n
            };
            // Reply / redaction relation handling.
            let reply_to_raw = content
                .and_then(|c| c.get("m.relates_to"))
                .and_then(|r| r.get("m.in_reply_to"))
                .and_then(|i| i.get("event_id"))
                .and_then(|e| e.as_str())
                .unwrap_or("");
            // threads: a thread message's m.in_reply_to with is_falling_back is only for clients without threads: no quote
            let thread_id = plan.tids[vi].as_str();
            let reply_to = if crate::threads_rows::fallback_quote(thread_id, content.and_then(|c| c.get("m.relates_to"))) { "" } else { reply_to_raw };
            let redacted = v.get("unsigned").and_then(|u| u.get("redacted_because")).is_some();
            // Strip the rich-reply quote fallback ("> <@user> …") from the shown body — only on a real reply (a thread
            // message's fallback m.in_reply_to has no quote text, review #10)
            let body_owned = if reply_to.is_empty() {
                body.to_string()
            } else {
                crate::embeds::strip_reply_fallback(body)
            };
            let body = body_owned.as_str();
            // Inline image: download + decrypt to a cached file:// (m.image only; other
            // media types show as a chip). Resolved AFTER the pagination loop with
            // bounded concurrency; record which message index needs it.
            let prefetch_audio =
                msgtype == "m.audio" && (size_bytes == 0 || size_bytes <= AUDIO_PREFETCH_MAX);
            // security review 2026-10-26: an image is fetched for the bubble only when it may be an inline image (an
            // inline type, or no type given — its bytes decide, viewer.rs) and not bigger than INLINE_IMAGE_MAX
            let prefetch_image = msgtype == "m.image"
                && (mimetype.is_empty() || crate::viewer::inline_image(mimetype))
                && size_bytes <= crate::viewer::INLINE_IMAGE_MAX;
            if prefetch_image || prefetch_audio {
                if let Some(c) = content {
                    pending_media.push((
                        msgs.len(),
                        c.clone(),
                        id.to_string(),
                        if mimetype.is_empty() { None } else { Some(mimetype.to_string()) },
                    ));
                }
            }
            // 0.8.33: a calendar announcement carries `dev.tinhat.calendar` {container, event}
            // (calendar.rs announce_content) — ChatBubble turns it into the live CalendarCard.
            let calref = content.and_then(|c| c.get("dev.tinhat.calendar"));
            let cal_container = calref.and_then(|r| r.get("container")).and_then(|x| x.as_str()).unwrap_or("");
            let cal_event = calref.and_then(|r| r.get("event")).and_then(|x| x.as_str()).unwrap_or("");
            // 11.0 (THEMES-UI.md §14): a theme share card — the ORIGINAL content's `dev.tinhat.theme.share`, read by
            // the contract's reader (the receiver resolves it itself: none / newer / refused / ok + swatches), cached
            // per event (a card is resolved once, not on every reload). ChatBubble shows ThemeShareCard for it.
            let theme_share = content.map_or(serde_json::Value::Null, |c| theme_share_of(id, c));
            // widgets W3 (WIDGETS.md §16): a dice roll — the ORIGINAL content's `dev.tinhat.dice`, read by the shared
            // reader (a card only when it re-adds up); ChatBubble shows DiceCard for it, the Dice widget's history reads it
            let dice = if redacted { serde_json::Value::Null } else { content.and_then(tinhat_widgets::dice::read_dice).map_or(serde_json::Value::Null, |r| crate::dice_ctl::card_json(&r)) };
            // 9.3: does this line address me? Explicit m.mentions (what our own composer and
            // Element send), @room, or my display name in the text — the same three things
            // the server's push rules highlight on, so the wash and the notification agree.
            let mentions = content.and_then(|c| c.get("m.mentions"));
            let mentions_me = sender != my_id
                && (mentions
                    .and_then(|m| m.get("user_ids"))
                    .and_then(|u| u.as_array())
                    .map_or(false, |a| a.iter().any(|x| x.as_str() == Some(my_id.as_str())))
                    || mentions.and_then(|m| m.get("room")).and_then(|r| r.as_bool()) == Some(true)
                    || {
                        let lc = body.to_lowercase();
                        lc.contains("@room") || (my_name_lc.len() >= 3 && lc.contains(&my_name_lc))
                    });
            let avatar = sender_avatar(room, sender, &mut avatars).await;
            // widgets W1: a pinned message's one-line snippet (the shared rule, recent::snippet_of) for the Pinned widget
            let pin_snippet = if pinned_ids.contains(id) {
                let ev = serde_json::json!({ "type": "m.room.message", "content": content.cloned().unwrap_or(serde_json::Value::Null), "redacted": redacted });
                tinhat_widgets::recent::snippet_of(&ev).to_json()
            } else {
                serde_json::Value::Null
            };
            let server_txn = v
                .get("unsigned")
                .and_then(|u| u.get("transaction_id"))
                .and_then(|t| t.as_str())
                .unwrap_or("");
            // threads: the summary under a root in the main timeline (names + faces resolved here, cached per load)
            let thread_json = match (view.is_empty() && !redacted).then(|| crate::threads_rows::root_thread(v, &plan.seen, &my_id, &thread_receipts, &plan.ts_of)).flatten() {
                Some(t) => {
                    let last_name = match names.get(&t.last_sender) {
                        Some(n) => n.clone(),
                        None => {
                            let n = resolve_member_name(room, &t.last_sender).await;
                            names.insert(t.last_sender.clone(), n.clone());
                            n
                        }
                    };
                    let mut repliers = Vec::new();
                    for u in &t.repliers {
                        let name = match names.get(u) {
                            Some(n) => n.clone(),
                            None => {
                                let n = resolve_member_name(room, u).await;
                                names.insert(u.clone(), n.clone());
                                n
                            }
                        };
                        let av = sender_avatar(room, u, &mut avatars).await;
                        repliers.push(serde_json::json!({ "id": u, "name": name, "avatar": av }));
                    }
                    serde_json::json!({ "count": t.count, "lastSender": t.last_sender, "lastName": last_name, "lastTs": t.last_ts,
                        "repliers": repliers, "unread": t.unread, "mine": t.mine })
                }
                None => serde_json::Value::Null,
            };
            // Inline media embeds (YouTube/Twitch/GIF/…) parsed from the body, plus
            // whether the body is only a media link (so QML can hide the raw URL).
            msgs.push(serde_json::json!({
                "sender": sender, "senderName": sender_name, "body": body, "ts": ts, "id": id,
                "avatar": avatar, "mentionsMe": mentions_me, "pinned": !redacted && pinned_ids.contains(id), // QA W1: no 📌 once deleted
                "serverTxn": server_txn,
                "msgtype": msgtype, "formatted": formatted, "media": "",
                "mxc": media_url, "mimetype": mimetype, "w": w, "h": h,
                "durationMs": duration_ms, "size": size_bytes,
                // voice notes: an MSC3245 voice message (ChatBubble's player reports its load for the interop gate)
                "voice": content.is_some_and(tinhat_widgets::voicenote::is_voice_note),
                // security review 2026-10-26: Open only for viewer types (viewer.rs: the OS opener) and text-like files
                // (shown in the app as plain text, launch-gate's web review #8); everything else is Save as only
                "canOpen": open_as(msgtype, !media_url.is_empty(), mimetype, &content.map(attachment_name).unwrap_or_default()) != OpenAs::SaveOnly,
                "replyTo": reply_to, "redacted": redacted,
                "embeds": crate::embeds::extract_embeds(body),
                "onlyEmbed": crate::embeds::body_is_only_embed(body),
                "calContainer": cal_container, "calEvent": cal_event,
                "themeShare": theme_share,
                "pinSnippet": pin_snippet,
                "dice": dice,
                // mini games v1 (GAMES-V1-SPEC.md §6.6): a game card's pointer block (kind, game, server, table, round); the
                // card is drawn from the room's own server's record (PlayController.cardView), never from these numbers
                "game": content.and_then(|c| c.get(crate::play_wire::CARD_KEY)).filter(|_| !redacted).filter(|g| crate::play_wire::card_of(&serde_json::json!({ crate::play_wire::CARD_KEY: g })).is_some()).cloned().unwrap_or(serde_json::Value::Null),
                // threads (docs/THREADS.md): the thread this message is in ("" = main) + the summary under a root
                "threadId": thread_id,
                "thread": thread_json,
            }));
            // 12.1.17: a photo posted by Memories' "Add photos" (its marker) is quiet: no receive sound
            if crate::memories_photos::post_event(v) {
                if let Some(m) = msgs.last_mut() {
                    m["quiet"] = serde_json::json!(true);
                }
            }
        }
    }
    drop(win); // derive is done with the window; media/receipt fetches below are slow
    // Resolve inline images with bounded concurrency (a fresh image-heavy room would
    // otherwise serialize N full-size downloads before anything paints), then fold the
    // file:// urls back in by index (buffer_unordered completes out of order).
    if !pending_media.is_empty() {
        use futures_util::StreamExt;
        let resolved: Vec<(usize, String)> = futures_util::stream::iter(pending_media)
            .map(|(i, c, id, mime)| async move {
                let purpose = if c.get("msgtype").and_then(|m| m.as_str()) == Some("m.image") { MediaUse::InlineImage } else { MediaUse::PrefetchAudio };
                (i, fetch_media(c, &id, mime.as_deref(), purpose).await.unwrap_or_default())
            })
            .buffer_unordered(4)
            .collect()
            .await;
        for (i, url) in resolved {
            msgs[i]["media"] = serde_json::json!(url);
        }
    }
    // Fold edits into their target messages (replace body/formatted, mark edited).
    for m in msgs.iter_mut() {
        if let Some(mid) = m.get("id").and_then(|i| i.as_str()).map(String::from) {
            if let Some((nb, nf)) = edits.get(&mid) {
                m["body"] = serde_json::json!(nb);
                // 10.1 (TH-MSG-08): an edit to plain text CLEARS the old rich text (it used to survive forever).
                    m["formatted"] = serde_json::json!(crate::richtext::message_html(nf));   // H2: an edit's HTML too
                m["edited"] = serde_json::json!(true);
            }
        }
    }
    // Resolve reply parents (sender + body snippet) from the loaded messages.
    let by_id: std::collections::HashMap<String, (String, String)> = msgs
        .iter()
        .filter_map(|m| {
            let id = m.get("id").and_then(|i| i.as_str())?.to_string();
            let sn = m.get("senderName").and_then(|s| s.as_str()).unwrap_or("").to_string();
            let b = m.get("body").and_then(|s| s.as_str()).unwrap_or("").to_string();
            Some((id, (sn, b)))
        })
        .collect();
    for m in msgs.iter_mut() {
        if let Some(rid) = m
            .get("replyTo")
            .and_then(|r| r.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from)
        {
            if let Some((sn, b)) = by_id.get(&rid) {
                m["replyToSender"] = serde_json::json!(sn);
                m["replyToBody"] = serde_json::json!(b);
            } else if let Some((_, sn)) = in_memories.get(&rid) {
                // 12.1.17: a reply to a photo the chat hides quotes it as "Photo in Memories" (a click opens it there)
                m["replyToSender"] = serde_json::json!(sn);
                m["replyToBody"] = serde_json::json!(tinhat_widgets::memphotos::QUOTE);
            }
        }
    }

    // Attach aggregated reactions to their target messages (chips render in QML).
    for m in msgs.iter_mut() {
        if let Some(mid) = m.get("id").and_then(|i| i.as_str()).map(|s| s.to_string()) {
            if let Some(newest_first) = reactions.get(&mid) {
                let oldest_first: Vec<crate::reactions::ReactIn> = newest_first.iter().rev().cloned().collect();
                m["reactions"] = serde_json::json!(crate::reactions::group_chips(&oldest_first, &my_id));
            }
        }
    }

    msgs.reverse(); // newest->oldest collected; show oldest->newest

    // Read receipts: attach, to each of the most recent messages, the members who've
    // read up to it (their avatar row renders bottom-right in QML). Local store reads,
    // capped to the tail so the sync hot-path stays cheap.
    //
    // NB: we do NOT send our OWN read receipt here. Doing so on every load created a
    // self-perpetuating loop (send receipt → server echoes m.receipt → RoomUpdate →
    // reload → send receipt …) and marked rooms read while merely open/scrolled-up.
    // Our read marker is sent explicitly from QML via mark_read() when the newest
    // message is actually visible at the bottom of the timeline.
    // 12.1.17 round 2 (review #8): a receipt resting on a photo the chat hides shows on the row before it (by position)
    attach_read_receipts(room, &mut msgs, &rows_newer).await;

    // 9.3: link previews — attach what the cache already knows; note what it does not
    // (the caller back-fills those off the load path). Bounded to the newest rows.
    let mut previews_wanted: Vec<(String, String)> = Vec::new();
    let first = msgs.len().saturating_sub(PREVIEW_TAIL);
    for m in msgs[first..].iter_mut() {
        if m.get("msgtype").and_then(|t| t.as_str()) != Some("m.text") || m.get("redacted") == Some(&serde_json::json!(true)) {
            continue;
        }
        let body = m.get("body").and_then(|b| b.as_str()).unwrap_or("");
        let Some(url) = crate::linkpreview::previewable_url(body) else { continue };
        match crate::linkpreview::cached(&url) {
            Some(Some(p)) => m["preview"] = p,
            Some(None) => {}
            None => {
                if let Some(id) = m.get("id").and_then(|i| i.as_str()) {
                    previews_wanted.push((id.to_string(), url));
                }
            }
        }
    }

    // 9.3: pinned messages present in the window, for the header popover. Widgets W1: + the sender's id and avatar, the
    // shared snippet (kind, text) and the pin's place in the shared order (pinIndex, 0 = the newest pin) for the widget.
    let pin_index = |id: &str| pin_order.iter().position(|p| p == id).map(|i| i as i64).unwrap_or(-1);
    // QA W1: a deleted pin stays a row with "deleted": true — the Pinned widget shows it as "Message deleted" (its
    // contract, as on the web); the composer's 📌 N and its list leave it out (ConversationView.qml, pins::shown_pins).
    let mut pinned_rows: Vec<serde_json::Value> = msgs
        .iter()
        .filter(|m| m.get("id").and_then(|i| i.as_str()).is_some_and(|id| pinned_ids.contains(id)))
        .map(|m| {
            let id = m.get("id").and_then(|i| i.as_str()).unwrap_or("");
            let sn = m.get("pinSnippet").cloned().unwrap_or_default();
            let deleted = m.get("redacted") == Some(&serde_json::json!(true));
            let text = m.get("body").and_then(|b| b.as_str()).unwrap_or("");
            serde_json::json!({
                "id": m.get("id").cloned().unwrap_or_default(),
                "senderName": m.get("senderName").cloned().unwrap_or_default(),
                "body": crate::pins::pin_row_body(false, deleted, text),
                "deleted": deleted,
                "ts": m.get("ts").cloned().unwrap_or_default(),
                "sender": m.get("sender").cloned().unwrap_or_default(),
                "avatar": m.get("avatar").cloned().unwrap_or_default(),
                "kind": sn.get("kind").cloned().unwrap_or_else(|| serde_json::json!("text")),
                "text": sn.get("text").cloned().unwrap_or_default(),
                "pinIndex": pin_index(id),
            })
        })
        .collect();
    // 10.1 (TH-TL-09): pins that have scrolled out of the loaded window are fetched once (cached
    // per event) so the noticeboard never loses entries as chat pushes them out; the rows are
    // in time order. A pin the view cannot reach still jumps via the "not loaded" toast.
    {
        let in_window: std::collections::HashSet<String> = pinned_rows
            .iter()
            .filter_map(|r| r.get("id").and_then(|i| i.as_str()).map(String::from))
            .collect();
        let mut older = Vec::new();
        for pid in pin_order.iter().filter(|p| !in_window.contains(*p)).take(20) {
            if let Some(mut row) = pinned_row_fetch(room, pid).await {
                row["pinIndex"] = serde_json::json!(pin_index(pid));
                older.push(row);
            }
        }
        pinned_rows.extend(older);
        pinned_rows.sort_by_key(|r| r.get("ts").and_then(|t| t.as_u64()).unwrap_or(0));
    }
    let pinned_json = serde_json::to_string(&pinned_rows).unwrap_or_else(|_| "[]".into());
    // 10.1 (TH-WP-04): is anyone watching a watch party in this room right now?
    let watch_party_on = {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        !crate::watchparty::read_viewers(room, now_ms).await.is_empty()
    };

    // The on-disk cache holds server rows only (a pending echo must not resurrect as a
    // ghost after a restart).
    // widgets W3: every poll's card from the whole window (Element's end rule: the creator, or an end sender who may
    // redact the start — here the redact power in the room's levels)
    let (poll_enders, poll_can_redact) = {
        let mut enders: Vec<String> = Vec::new();
        let mut mine = false;
        if let Ok(pl) = room.power_levels().await {
            mine = pl.user_can_redact_event_of_other(room.own_user_id());
            for r in &poll_rels {
                let (Some(ty), Some(sender)) = (r.get("type").and_then(|t| t.as_str()), r.get("sender").and_then(|s| s.as_str())) else { continue };
                if !tinhat_widgets::polls::END_TYPES.contains(&ty) || enders.iter().any(|e| e == sender) {
                    continue;
                }
                if let Ok(u) = matrix_sdk::ruma::UserId::parse(sender) {
                    if pl.user_can_redact_event_of_other(&u) {
                        enders.push(sender.to_string());
                    }
                }
            }
        }
        (enders, mine)
    };
    attach_poll_cards(&mut msgs, &poll_rels, &poll_enders, &my_id, poll_can_redact);
    if view.is_empty() {
        let cache_json = serde_json::to_string(&msgs).map_err(|e| e.to_string())?;
        write_timeline_cache(room.room_id().as_str(), &cache_json);
    }

    // widgets W3: the gates' sample rows (TINHAT_CHAT_SAMPLES=dice): LOCAL rows after the real ones — never sent, never cached
    for row in chat_sample_rows(&my_id, &my_name) {
        msgs.push(row);
    }

    // 9.3: local echoes. An echo whose event is in the window now (by the id the send
    // returned, by the server-side transaction id, or — for uploads — by sender + body
    // + time) is delivered and dropped; the rest render at the tail as pending / sent /
    // failed rows, in the order they were sent.
    let own_avatar = sender_avatar(room, &my_id, &mut avatars).await;
    // 12.1.17: what the chat hid this load (a jump to one opens Memories; a newest one still takes the read receipt)
    crate::memories_photos::note_hidden(room.room_id().as_str(), in_memories.keys().cloned(), view.is_empty());
    // the gates' hook (TINHAT_GUI_MEMHIDE=1, never set in a real launch): which image rows this load shows / hides.
    // Compiled only outside a release (cfg tinhat_env_selftests, build.rs; launch-gate 11-15 16:49: no new release env
    // switches): a release build never reads the variable, and scripts/release-binary-check.sh refuses one that names it.
    #[cfg(tinhat_env_selftests)] // never in a release (build.rs): a gate hook
    if std::env::var_os("TINHAT_GUI_MEMHIDE").is_some() {
        let shown: Vec<&str> = msgs.iter().filter(|m| m.get("msgtype").and_then(|t| t.as_str()) == Some("m.image")).filter_map(|m| m.get("id").and_then(|i| i.as_str())).collect();
        let mut hid: Vec<&str> = in_memories.keys().map(String::as_str).collect();
        hid.sort();
        let quiet: Vec<&str> = msgs.iter().filter(|m| m.get("quiet").and_then(|q| q.as_bool()) == Some(true)).filter_map(|m| m.get("id").and_then(|i| i.as_str())).collect();
        let quotes: Vec<String> = msgs
            .iter()
            .filter(|m| m.get("replyToBody").and_then(|b| b.as_str()) == Some(tinhat_widgets::memphotos::QUOTE))
            .filter_map(|m| m.get("id").and_then(|i| i.as_str()).map(|i| format!("{i}>{}", m.get("replyTo").and_then(|r| r.as_str()).unwrap_or(""))))
            .collect();
        let seen: Vec<String> = msgs
            .iter()
            .filter_map(|m| {
                let r = m.get("receipts")?.as_array()?;
                let who: Vec<&str> = r.iter().filter_map(|x| x.get("userId").and_then(|u| u.as_str())).collect();
                Some(format!("{}:{}", m.get("id")?.as_str()?, who.join("+")))
            })
            .collect();
        eprintln!(
            "[memhide] room={} view={} images=[{}] hidden=[{}] quiet=[{}] quotes=[{}] seen=[{}] index={}",
            room.room_id(),
            if view.is_empty() { "main" } else { view },
            shown.join(","),
            hid.join(","),
            quiet.join(","),
            quotes.join(","),
            seen.join(","),
            if mem_idx.is_some() { "ok" } else { "none" }
        );
    }
    // round 2 (review #2/#3): for the thread's panel too (keyed by its root), and the view's read tail: the hidden photo
    // when it is newer than every row (QML marks read when it moves while the view sits at its end)
    let newest_row_ts = msgs.iter().filter(|m| m.get("echo").and_then(|e| e.as_bool()) != Some(true)).filter_map(|m| m.get("ts").and_then(|t| t.as_u64())).max().unwrap_or(0);
    let read_tail = match &hidden_newest {
        Some((hid, _)) if hidden_is_tail => hid.clone(),
        _ => String::new(),
    };
    if let Some((hid, hts)) = hidden_newest.take() {
        // the timeline's order wins over equal (or skewed) clocks: a tail is stored newer than every row
        let ts = if hidden_is_tail { hts.max(newest_row_ts + 1) } else { hts.min(newest_row_ts.saturating_sub(1)) };
        note_nonrow(&nonrow_key(room.room_id().as_str(), view), hid, ts);
    }
    // an echo of a hidden photo is delivered too (its row is in Memories, not missing)
    let echoes = if hidden_stubs.is_empty() {
        echo_store_undelivered(room.room_id().as_str(), &msgs, view)
    } else {
        let mut all = msgs.clone();
        all.extend(hidden_stubs);
        echo_store_undelivered(room.room_id().as_str(), &all, view)
    };
    if !echoes.is_empty() {
        let cur = serde_json::to_string(&msgs).unwrap_or_else(|_| "[]".into());
        for e in echoes {
            msgs.push(echo_row(&e, &my_id, &my_name, &own_avatar, &cur));
        }
    }

    // widgets W3: the dice rows (server rows + my echoes) for the Dice widget's room history; the open polls for Polls
    let dice_json = dice_rows(&msgs).to_string();
    let polls_json = poll_rows(&msgs).to_string();
    let json = serde_json::to_string(&msgs).map_err(|e| e.to_string())?;
    Ok(Loaded {
        name,
        json,
        call_active,
        has_more,
        game_code,
        pinned_json,
        pinned_count: pin_order.len() as i32,
        dice_json,
        polls_json,
        poll_rels,
        poll_enders,
        poll_can_redact,
        watch_party_on,
        own_name: my_name,
        own_avatar,
        previews_wanted,
        read_tail,
    })
}

/// How many newest rows get a link-preview lookup per load.
const PREVIEW_TAIL: usize = 60;

/// A sender's profile picture as a local file:// (cached per load; the avatar cache on
/// disk makes repeat resolves a stat). "" when they have none.
pub(crate) async fn sender_avatar(
    room: &Room,
    mxid: &str,
    cache: &mut std::collections::HashMap<String, String>,
) -> String {
    if let Some(a) = cache.get(mxid) {
        return a.clone();
    }
    let mut out = String::new();
    if let (Ok(uid), Some(client)) = (matrix_sdk::ruma::UserId::parse(mxid), crate::session::client()) {
        if let Ok(Some(m)) = room.get_member_no_sync(&uid).await {
            if let Some(mxc) = m.avatar_url() {
                out = crate::bridge::resolve_avatar(&client, mxc).await.unwrap_or_default();
            }
        }
    }
    cache.insert(mxid.to_string(), out.clone());
    out
}

/// Attach read-receipt reader names ({userId,name}) to the tail messages — i.e. which
/// members have read up to each of the most recent messages. Local store reads only.
/// 12.1.17 round 2 (review #8, r2 #6): the row a receipt on a hidden photo shows on: the row just before it in the
/// timeline (`rows`, oldest first; `newer` = how many rows are newer than the photo), as the web maps a non-row event's
/// receipt to the row before it. None = no row before it is loaded.
fn row_before(rows: usize, newer: usize) -> Option<usize> {
    rows.checked_sub(newer)?.checked_sub(1)
}

async fn attach_read_receipts(room: &Room, msgs: &mut [serde_json::Value], hidden: &[(String, usize)]) {
    use matrix_sdk::ruma::events::receipt::{ReceiptThread, ReceiptType};
    let my_id = room.own_user_id().to_string();
    let n = msgs.len();
    let start = n.saturating_sub(RECEIPT_TAIL);
    let mut names: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut avatars: std::collections::HashMap<String, String> = std::collections::HashMap::new();   // 10.1 (TH-MSG-20)
    for m in msgs[start..].iter_mut() {
        let Some(id) = m.get("id").and_then(|i| i.as_str()).map(String::from) else {
            continue;
        };
        let Ok(eid) = matrix_sdk::ruma::EventId::parse(&id) else {
            continue;
        };
        let readers = match room
            .load_event_receipts(ReceiptType::Read, &ReceiptThread::Unthreaded, &eid)
            .await
        {
            Ok(r) => r,
            Err(_) => continue,
        };
        let mut arr: Vec<serde_json::Value> = Vec::new();
        for (uid, _receipt) in readers {
            let uid_s = uid.to_string();
            if uid_s == my_id {
                continue; // don't show our own read marker
            }
            let name = if let Some(nm) = names.get(&uid_s) {
                nm.clone()
            } else {
                let nm = resolve_member_name(room, &uid_s).await;
                names.insert(uid_s.clone(), nm.clone());
                nm
            };
            // 10.1 (TH-MSG-20): the reader's picture too, so "seen by" shows faces like everything else.
            let av = sender_avatar(room, &uid_s, &mut avatars).await;
            arr.push(serde_json::json!({ "userId": uid_s, "name": name, "avatar": av }));
        }
        if !arr.is_empty() {
            m["receipts"] = serde_json::json!(arr);
        }
    }
    // 12.1.17 round 2 (review #8): readers whose receipt rests on a photo the chat hides show on the row before it
    let rows = msgs.len();
    for (hid, newer) in hidden {
        let Some(i) = row_before(rows, *newer).filter(|i| *i >= start) else { continue };
        let Ok(eid) = matrix_sdk::ruma::EventId::parse(hid) else { continue };
        let Ok(readers) = room.load_event_receipts(ReceiptType::Read, &ReceiptThread::Unthreaded, &eid).await else { continue };
        for (uid, _receipt) in readers {
            let uid_s = uid.to_string();
            if uid_s == my_id {
                continue;
            }
            let have = msgs[i].get("receipts").and_then(|r| r.as_array()).is_some_and(|a| a.iter().any(|r| r.get("userId").and_then(|u| u.as_str()) == Some(uid_s.as_str())));
            if have {
                continue;
            }
            let name = if let Some(nm) = names.get(&uid_s) {
                nm.clone()
            } else {
                let nm = resolve_member_name(room, &uid_s).await;
                names.insert(uid_s.clone(), nm.clone());
                nm
            };
            let av = sender_avatar(room, &uid_s, &mut avatars).await;
            let entry = serde_json::json!({ "userId": uid_s, "name": name, "avatar": av });
            match msgs[i].get_mut("receipts").and_then(|r| r.as_array_mut()) {
                Some(a) => a.push(entry),
                None => msgs[i]["receipts"] = serde_json::json!([entry]),
            }
        }
    }
}

/// On-disk timeline cache dir: `$XDG_CACHE_HOME/Tin Hat/timeline`. Lets a room
/// paint its last-seen messages instantly on open, before the live load completes.
pub(crate) fn timeline_cache_dir() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache")
        });
    base.join("TinHat/timeline")
}

fn timeline_cache_path(room_id: &str) -> std::path::PathBuf {
    timeline_cache_dir().join(format!("{}.json", sanitize_id(room_id)))
}

/// The room's timeline snapshot + its row count, if there is a READABLE one.
fn read_timeline_cache(room_id: &str) -> Option<(String, usize)> {
    read_snapshot_at(&timeline_cache_path(room_id))
}

/// 10.5.2 (timeline): a snapshot is painted only if it parses as a JSON array. A torn or corrupt
/// file used to go straight to QML, whose JSON.parse threw inside the mirror — the timeline stayed
/// blank until the live load replaced the string (and for good if that load failed). A bad file
/// is deleted so the next open doesn't trip on it again.
fn read_snapshot_at(path: &std::path::Path) -> Option<(String, usize)> {
    let s = std::fs::read_to_string(path).ok()?;
    if s.trim().is_empty() {
        return None;
    }
    match serde_json::from_str::<Vec<serde::de::IgnoredAny>>(&s) {
        Ok(rows) => Some((s, rows.len())),
        Err(e) => {
            eprintln!("[room] timeline snapshot {} unreadable ({e}) — discarded", path.display());
            let _ = std::fs::remove_file(path);
            None
        }
    }
}

fn write_timeline_cache(room_id: &str, json: &str) {
    let path = timeline_cache_path(room_id);
    let json = json.to_string();
    // Fire-and-forget on the blocking pool — never stall an async worker on disk IO.
    let _ = crate::bridge::runtime().spawn_blocking(move || {
        if let Err(e) = write_snapshot_atomic(&path, &json) {
            eprintln!("[room] timeline snapshot write failed ({}): {e}", path.display());
        }
    });
}

/// 10.5.2 (timeline): write-to-temp + rename, one writer at a time. `std::fs::write` truncates
/// the file in place, so the next open() could read it half-written, and two writers — the main
/// view and a floating chat of the same room reload on the same sync — could interleave into
/// invalid JSON on disk. A rename replaces the file whole (POSIX and Windows alike).
fn write_snapshot_atomic(path: &std::path::Path, json: &str) -> std::io::Result<()> {
    static WRITE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one_at_a_time = WRITE.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// On-disk members/summary cache dir, mirroring the timeline cache: lets the right
/// panel + header subtitle paint the last-seen member list instantly on open, before
/// the (slower) live member sync + avatar resolution completes.
pub(crate) fn members_cache_dir() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache")
        });
    base.join("TinHat/members")
}

fn members_cache_path(room_id: &str) -> std::path::PathBuf {
    members_cache_dir().join(format!("{}.json", sanitize_id(room_id)))
}

/// Returns (members_json, summary_json) from the cache, or None.
fn read_members_cache(room_id: &str) -> Option<(String, String)> {
    let raw = std::fs::read_to_string(members_cache_path(room_id)).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let m = serde_json::to_string(v.get("members")?).ok()?;
    let s = serde_json::to_string(v.get("summary")?).ok()?;
    Some((m, s))
}

fn write_members_cache(room_id: &str, members_json: &str, summary_json: &str) {
    let members_json = members_json.to_string();
    let summary_json = summary_json.to_string();
    let dir = members_cache_dir();
    let path = members_cache_path(room_id);
    // Fire-and-forget on the blocking pool — never stall an async worker on disk IO.
    let _ = crate::bridge::runtime().spawn_blocking(move || {
        let m: serde_json::Value = serde_json::from_str(&members_json).unwrap_or_else(|_| serde_json::json!([]));
        let s: serde_json::Value = serde_json::from_str(&summary_json).unwrap_or_else(|_| serde_json::json!({}));
        let wrapper = serde_json::json!({ "members": m, "summary": s });
        if std::fs::create_dir_all(&dir).is_ok() {
            let _ = std::fs::write(path, wrapper.to_string());
        }
    });
}

/// Map a file extension to a MIME type string (covers the common image/video/audio/doc
/// types; the SDK picks the right msgtype — m.image/m.video/m.audio/m.file — from it).
pub(crate) fn mime_for_ext(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        // phone photos: sent as pictures (like the web, whose browser types them), so they lose their location too
        // (docs/PHOTO-METADATA.md). Not drawn inline (no shipped Qt reads them): a file card, as for any non-inline image.
        "heic" => "image/heic",
        "heif" | "hif" => "image/heif",
        "avif" => "image/avif",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "ogg" | "oga" => "audio/ogg",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "m4a" => "audio/mp4",
        "opus" => "audio/opus",
        "pdf" => "application/pdf",
        "txt" | "log" | "md" => "text/plain",
        "json" => "application/json",
        "zip" => "application/zip",
        "7z" => "application/x-7z-compressed",
        "tar" => "application/x-tar",
        "gz" => "application/gzip",
        _ => "application/octet-stream",
    }
}

/// Send a VERBATIM plain-text message (no markdown processing). Used by the gif picker
/// so a CDN URL containing `_`/`-`/`~` is never mangled by markdown emphasis — the body
/// must stay an exact URL for the inline embed parser (and the web fork) to render it.
pub(crate) async fn do_send_plain(room_id: &str, text: &str) -> Result<(), String> {
    let client = crate::session::client().ok_or_else(|| "Not signed in".to_string())?;
    let rid = RoomId::parse(room_id).map_err(|e| format!("bad room id: {e}"))?;
    let room = client
        .get_room(&rid)
        .ok_or_else(|| "Room not found".to_string())?;
    room.send(RoomMessageEventContent::text_plain(text))
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Minimal HTML escape for building a `formatted_body`.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Build the `formatted_body` HTML for a mention message: escape the body, swap each
/// `@name` for a placeholder (so the anchor we insert — which itself contains `@name` —
/// can't be re-matched), convert newlines to `<br/>`, then expand placeholders into
/// `matrix.to` anchors. Pure (no Matrix/network) so it's unit-tested.
async fn empty_room_receipt(room_id: &str) {
    type Seen = std::collections::HashMap<String, std::time::Instant>;
    static LAST: std::sync::OnceLock<std::sync::Mutex<Seen>> = std::sync::OnceLock::new();
    let Some(client) = crate::session::client() else { return };
    let Ok(rid) = matrix_sdk::ruma::RoomId::parse(room_id) else { return };
    let Some(room) = client.get_room(&rid) else { return };
    if room.unread_notification_counts().notification_count == 0 {
        return;
    }
    {
        let mut m = LAST.get_or_init(|| std::sync::Mutex::new(Seen::new())).lock().unwrap_or_else(|p| p.into_inner());
        if m.get(room_id).is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(60)) {
            return;
        }
        m.insert(room_id.to_string(), std::time::Instant::now());
    }
    crate::nav::mark_rooms_read(&client, vec![room_id.to_string()]).await;
}

/// 10.6 (REACH A5.2): who a reply mentions — the picks as typed, then the parent's sender;
/// never yourself, no duplicates. Pure (the web mirrors it: `reply_mentions`).
pub(crate) fn reply_mention_ids(parent_sender: Option<&str>, me: &str, picked: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for u in picked.iter().map(String::as_str).chain(parent_sender) {
        if u.is_empty() || u == me || out.iter().any(|x| x == u) {
            continue;
        }
        out.push(u.to_string());
    }
    out
}

/// The sender (mxid) of the loaded timeline row `id`, "" when it is not loaded.
/// threads: the newest row of a timeline that is a server event (not a local echo), for a thread send's fallback reply.
fn newest_server_row(messages_json: &str) -> Option<String> {
    let v = serde_json::from_str::<serde_json::Value>(messages_json).ok()?;
    v.as_array()?
        .iter()
        .rev()
        .filter(|m| m.get("echo").and_then(|e| e.as_bool()) != Some(true))
        .find_map(|m| m.get("id").and_then(|i| i.as_str()).filter(|i| i.starts_with('$')).map(str::to_string))
}

/// A member's display name through a per-call cache.
async fn cached_name(room: &Room, names: &mut std::collections::HashMap<String, String>, u: &str) -> String {
    if let Some(n) = names.get(u) {
        return n.clone();
    }
    let n = resolve_member_name(room, u).await;
    names.insert(u.to_string(), n.clone());
    n
}

/// threads: the Threads list's JSON (see RoomController::load_threads): the server's /threads roots (each with its
/// bundled summary) + the roots the room's window holds replies for, summarised like the timeline's roots.
async fn load_threads_json(room: &Room, pages: u32) -> String {
    use matrix_sdk::room::ListThreadsOptions;
    use tinhat_widgets::threads::{thread_list, ListThread};
    let me = room.own_user_id().to_string();
    let mut roots: Vec<serde_json::Value> = Vec::new();
    let mut failed = false;
    // `pages` pages of 50 (Load more threads); `more` = the server has older ones
    let mut from: Option<String> = None;
    let mut more = false;
    for _ in 0..pages.max(1) {
        let mut opts = ListThreadsOptions::default();
        opts.limit = Some(UInt::from(50u32));
        opts.from = from.clone();
        match room.list_threads(opts).await {
            Ok(page) => {
                for ev in page.chunk.iter() {
                    if let Ok(mut v) = ev.raw().deserialize_as::<serde_json::Value>() {
                        crate::moved::normalize(&mut v);
                        roots.push(v);
                    }
                }
                from = page.prev_batch_token.clone();
                more = from.is_some();
                if !more {
                    break;
                }
            }
            Err(e) => {
                eprintln!("[threads] /threads failed: {e}");
                failed = true;
                break;
            }
        }
    }
    let window: Vec<serde_json::Value> = timeline_window_for(room.room_id().as_str()).lock().await.events.clone();
    let plan = crate::threads_rows::thread_plan(&window, None);
    // roots the window has replies for but the server didn't list (yet): the window's copy, or a bare entry
    for r in plan.seen.keys() {
        if roots.iter().any(|v| crate::threads_rows::ev_id(v) == r) {
            continue;
        }
        match window.iter().find(|v| crate::threads_rows::ev_id(v) == r) {
            Some(v) => roots.push(v.clone()),
            None => roots.push(serde_json::json!({ "event_id": r })),
        }
    }
    let ids: Vec<String> = roots.iter().map(|v| crate::threads_rows::ev_id(v).to_string()).collect();
    let receipts = crate::receipts::my_thread_receipts(room, &ids).await;
    let mut names: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut avatars: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut items = Vec::new();
    let mut list = Vec::new();
    // 12.1.17: a photo in Memories isn't listed (its comments live in Memories' viewer, which opens its thread)
    let mem_idx = crate::memories_photos::chat_index(room).await;
    for v in &roots {
        if crate::memories_photos::hidden_event(mem_idx.as_ref(), v) {
            continue;
        }
        let Some(t) = crate::threads_rows::root_thread(v, &plan.seen, &me, &receipts, &plan.ts_of) else { continue };
        let root = crate::threads_rows::ev_id(v).to_string();
        let sender = v.get("sender").and_then(|s| s.as_str()).unwrap_or("").to_string();
        let root_name = if sender.is_empty() { String::new() } else { cached_name(room, &mut names, &sender).await };
        let last_name = cached_name(room, &mut names, &t.last_sender).await;
        let avatar = if sender.is_empty() { String::new() } else { sender_avatar(room, &sender, &mut avatars).await };
        let redacted = v.get("unsigned").and_then(|u| u.get("redacted_because")).is_some();
        let body = v.get("content").and_then(|c| c.get("body")).and_then(|b| b.as_str()).unwrap_or("");
        let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let snippet = if redacted {
            "Message deleted".to_string()
        } else if ty == "m.room.encrypted" {
            "Encrypted message".to_string()
        } else if ty.is_empty() {
            "Message not loaded".to_string()
        } else {
            body.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(200).collect()
        };
        list.push(ListThread { root: root.clone(), root_sender: sender.clone(), last_ts: t.last_ts, mine: t.mine });
        items.push(serde_json::json!({ "root": root, "rootSender": sender, "rootName": root_name, "rootAvatar": avatar,
            "snippet": snippet, "count": t.count, "lastName": last_name, "lastTs": t.last_ts, "unread": t.unread, "mine": t.mine }));
    }
    serde_json::json!({ "items": items, "all": thread_list(&list, "all", &me), "mine": thread_list(&list, "mine", &me), "error": failed, "more": more }).to_string()
}

/// threads: the thread a loaded row is in ("" = main or not loaded).
fn row_thread(messages_json: &str, id: &str) -> String {
    serde_json::from_str::<serde_json::Value>(messages_json)
        .ok()
        .and_then(|v| {
            v.as_array()?
                .iter()
                .find(|m| m.get("id").and_then(|i| i.as_str()) == Some(id))
                .and_then(|m| m.get("threadId").and_then(|s| s.as_str()).map(str::to_string))
        })
        .unwrap_or_default()
}

/// threads: one thread's /relations pages for its panel (fetched once, then page by page as the panel scrolls up) and
/// its root, per room + thread. Newest page first; `next` = the server's token for older ones (None = the start).
#[derive(Default)]
struct ThreadWindow {
    fetched: Vec<serde_json::Value>,
    root: Option<serde_json::Value>,
    next: Option<String>,
    started: bool,
    /// review #2: when the newest page was last fetched (a thread reopened later fetches its head again)
    head_at: Option<std::time::Instant>,
}

fn thread_windows() -> &'static std::sync::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<ThreadWindow>>>> {
    static W: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<ThreadWindow>>>>> = std::sync::OnceLock::new();
    W.get_or_init(Default::default)
}

/// threads: the thread's events (newest first, threads_rows::merge_thread) for `want` rows, and whether older ones may
/// exist. Network: the first page on the first load, then one more page whenever `want` outgrows what is loaded.
async fn thread_window_events(room: &Room, root: &str, want: usize, window: &[serde_json::Value], session_gen: u64) -> (Vec<serde_json::Value>, bool) {
    use matrix_sdk::room::RelationsOptions;
    // review #1 / round 2 #12: per session (a signed-out account's decrypted thread never reaches the next one) — the
    // generation the load STARTED in, and nothing is kept once the session changed meanwhile
    if crate::session::generation() != session_gen {
        return (Vec::new(), false);
    }
    let key = format!("{session_gen}|{}|{root}", room.room_id());
    let tw = thread_windows().lock().unwrap_or_else(|p| p.into_inner()).entry(key).or_default().clone();
    let mut tw = tw.lock().await;
    let Ok(root_id) = matrix_sdk::ruma::OwnedEventId::try_from(root) else { return (Vec::new(), false) };
    if tw.root.is_none() && !window.iter().any(|v| crate::threads_rows::ev_id(v) == root) {
        if let Ok(ev) = Box::pin(room.load_or_fetch_event(&root_id, None)).await {
            if let Ok(mut v) = ev.raw().deserialize_as::<serde_json::Value>() {
                crate::moved::normalize(&mut v);
                tw.root = Some(v);
            }
        }
    }
    // the thread's own messages (is_rendered_row leaves thread replies out: it counts the main timeline's rows)
    let rows = |fetched: &[serde_json::Value], rt: Option<&serde_json::Value>| {
        crate::threads_rows::merge_thread(root, window, fetched, rt)
            .iter()
            .filter(|v| {
                matches!(v.get("type").and_then(|t| t.as_str()), Some("m.room.message") | Some("m.room.encrypted"))
                    && v.get("content").and_then(|c| c.get("m.relates_to")).and_then(|r| r.get("rel_type")).and_then(|r| r.as_str()) != Some("m.replace")
            })
            .count()
    };
    // review #2: the newest page again when it is older than 30 s (replies missed while the panel was closed, or that
    // the room's window has dropped since), merged in by id
    if tw.started && tw.head_at.is_none_or(|t| t.elapsed() > std::time::Duration::from_secs(30)) {
        // round 2 #2: newest pages until one overlaps what is held (a gap longer than a page fills too), at most 6
        let mut from: Option<String> = None;
        for _ in 0..6 {
            let mut opts = RelationsOptions::default();
            opts.from = from.clone();
            opts.limit = Some(UInt::from(50u32));
            opts.recurse = true;
            let Ok(page) = room.relations(root_id.clone(), opts).await else { break };
            if crate::session::generation() != session_gen {
                return (Vec::new(), false);
            }
            let mut overlap = false;
            for ev in page.chunk.iter() {
                if let Ok(mut v) = ev.raw().deserialize_as::<serde_json::Value>() {
                    crate::moved::normalize(&mut v);
                    let id = crate::threads_rows::ev_id(&v).to_string();
                    match tw.fetched.iter().position(|f| crate::threads_rows::ev_id(f) == id) {
                        Some(i) => {
                            overlap = true;
                            // round 2 #14: a copy that decrypts now replaces the one held without its key
                            let was_utd = tw.fetched[i].get("type").and_then(|t| t.as_str()) == Some("m.room.encrypted");
                            if was_utd && v.get("type").and_then(|t| t.as_str()) != Some("m.room.encrypted") {
                                tw.fetched[i] = v;
                            }
                        }
                        None => tw.fetched.push(v),
                    }
                }
            }
            tw.head_at = Some(std::time::Instant::now());
            if overlap || page.next_batch_token.is_none() {
                break;
            }
            from = page.next_batch_token.clone();
        }
    }
    // at most a few pages per pass (a long thread fills in as the panel scrolls)
    for _ in 0..4 {
        let need = !tw.started || (tw.next.is_some() && rows(&tw.fetched, tw.root.as_ref()) < want);
        if !need {
            break;
        }
        let mut opts = RelationsOptions::default();
        opts.from = if tw.started { tw.next.clone() } else { None };
        opts.limit = Some(UInt::from(50u32));
        opts.recurse = true;
        match room.relations(root_id.clone(), opts).await {
            Ok(page) => {
                if crate::session::generation() != session_gen {
                    return (Vec::new(), false);
                }
                for ev in page.chunk.iter() {
                    if let Ok(mut v) = ev.raw().deserialize_as::<serde_json::Value>() {
                        crate::moved::normalize(&mut v);
                        if let matrix_sdk::deserialized_responses::TimelineEventKind::UnableToDecrypt { utd_info, .. } = &ev.kind {
                            crate::utd::annotate(&mut v, &utd_info.reason);
                        }
                        tw.fetched.push(v);
                    }
                }
                // dir=b: the server's next_batch continues further back (the spec's token for "more")
                tw.next = page.next_batch_token.clone();
                if !tw.started {
                    tw.head_at = Some(std::time::Instant::now());
                }
                tw.started = true;
            }
            Err(e) => {
                eprintln!("[threads] relations of {root} failed: {e}");
                break;
            }
        }
    }
    // review #3: the window's redactions also reach the copies fetched here (the window stamps only its own)
    for red in window.iter().filter(|v| v.get("type").and_then(|t| t.as_str()) == Some("m.room.redaction")) {
        let target = red
            .get("redacts")
            .and_then(|r| r.as_str())
            .or_else(|| red.get("content").and_then(|c| c.get("redacts")).and_then(|r| r.as_str()))
            .unwrap_or("")
            .to_string();
        let twm = &mut *tw;
        for v in twm.fetched.iter_mut().chain(twm.root.iter_mut()) {
            if crate::threads_rows::ev_id(v) == target && v.get("unsigned").and_then(|u| u.get("redacted_because")).is_none() {
                stamp_redacted(v, red);
            }
        }
    }
    let evs = crate::threads_rows::merge_thread(root, window, &tw.fetched, tw.root.as_ref());
    (evs, tw.next.is_some() || !tw.started)
}

fn row_sender(messages_json: &str, id: &str) -> String {
    serde_json::from_str::<serde_json::Value>(messages_json)
        .ok()
        .and_then(|v| {
            v.as_array()?
                .iter()
                .find(|m| m.get("id").and_then(|i| i.as_str()) == Some(id))
                .and_then(|m| m.get("sender").and_then(|s| s.as_str()).map(str::to_string))
        })
        .filter(|s| s.starts_with('@'))
        .unwrap_or_default()
}

/// The sender of the event being replied to (the room's event cache, else one /event request).
async fn reply_parent_sender(room: &matrix_sdk::Room, parent_id: &str) -> Option<String> {
    let eid = matrix_sdk::ruma::EventId::parse(parent_id).ok()?;
    let ev = room.event(&eid, None).await.ok()?;
    ev.raw().get_field::<String>("sender").ok().flatten()
}

fn render_mention_html(text: &str, mentions: &[(String, String)]) -> String {
    let mut html = html_escape(text);
    let mut links: Vec<(String, String)> = Vec::new(); // (placeholder, anchor html)
    for (i, (name, uid)) in mentions.iter().enumerate() {
        let needle = format!("@{}", html_escape(name));
        if html.contains(&needle) {
            let ph = format!("\u{1}M{i}\u{1}");
            html = html.replace(&needle, &ph);
            let anchor = format!("<a href=\"https://matrix.to/#/{uid}\">@{}</a>", html_escape(name));
            links.push((ph, anchor));
        }
    }
    let mut html = html.replace('\n', "<br/>");
    for (ph, anchor) in &links {
        html = html.replace(ph, anchor);
    }
    html
}

/// On-disk cache dir for decrypted inline media: `$XDG_CACHE_HOME/Tin Hat/media`.
pub(crate) fn media_cache_dir() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache")
        });
    base.join("TinHat/media")
}

/// pinned_row_fetch's rows (event id → row) for this session.
fn pinned_rows() -> &'static std::sync::Mutex<std::collections::HashMap<String, serde_json::Value>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, serde_json::Value>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// theme_share_of's cards (event id → card) for this session.
fn theme_shares() -> &'static std::sync::Mutex<std::collections::HashMap<String, serde_json::Value>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, serde_json::Value>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// Sign-out (signout.rs): nothing of this session's rooms stays in memory for the next one: the timeline windows
/// (their decrypted events), local echoes, pinned rows and theme cards, which rooms' keys arrived and whose key
/// backups were pulled (the next session pulls them again), the newest non-row marks. The disk caches: signout.rs.
pub(crate) fn forget_session() {
    timeline_windows().lock().unwrap_or_else(|p| p.into_inner()).clear();
    echo_store().lock().unwrap_or_else(|p| p.into_inner()).clear();
    keys_epochs().lock().unwrap_or_else(|p| p.into_inner()).clear();
    backups_done().lock().unwrap_or_else(|p| p.into_inner()).clear();
    newest_nonrow().lock().unwrap_or_else(|p| p.into_inner()).clear();
    crate::memories_photos::forget_hidden(); // 12.1.17: the photos the chat hid
    pinned_rows().lock().unwrap_or_else(|p| p.into_inner()).clear();
    theme_shares().lock().unwrap_or_else(|p| p.into_inner()).clear();
    thread_windows().lock().unwrap_or_else(|p| p.into_inner()).clear(); // threads: their fetched events
}


fn sanitize_id(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect()
}

/// Download + decrypt an `m.image` message's media to a cache file, returning a
/// `file://` url QML can load. Cached by event id (and the SDK's own media cache), so
/// after the first load this is just a `Path::exists` check — no network/decrypt.
/// What a pasted link points at.
///
/// Split out from the joining so the messy half — the shapes people actually paste —
/// can be unit-tested without a homeserver. Links arrive from chat clients, from the
/// address bar, and hand-typed, and every one of those mangles them differently.
#[derive(Debug, PartialEq, Eq)]
pub enum JoinTarget {
    /// A room or space: `!id:server` or `#alias:server`, plus any `via=` routing hints.
    RoomOrAlias { id: String, via: Vec<String> },
    /// A person. Not a space, but people paste these, so answer usefully.
    Person(String),
}

/// `!room:server/$event` → the room: cut at the first `/$` whose head is a whole room (`!id:server`, `#alias:server` or
/// a v12 `!opaque`). An alias may itself hold `/$` (`#a/$b:server`, legal): its `#a` is no room, so it stays whole
/// (assembly-037 D3; the web's joinLink.ts cutAtEvent is the same rule).
fn cut_at_event(t: &str) -> &str {
    use crate::share_link::js_space;
    if !(t.starts_with('!') || t.starts_with('#')) {
        return t;
    }
    let mut from = 0;
    while let Some(off) = t[from..].find("/$") {
        let i = from + off;
        if i > 0 {
            let head = &t[..i];
            let with_server = head[1..].split_once(':').is_some_and(|(l, r)| !l.is_empty() && !r.is_empty());
            let v12 = head.starts_with('!') && head.len() > 1 && !head.contains(':') && !head.contains(js_space);
            if with_server || v12 {
                return head;
            }
        }
        from = i + 1;
    }
    t
}

/// Parse anything a user might paste into "join with a link".
pub fn parse_join_link(input: &str) -> Result<JoinTarget, String> {
    use crate::share_link::{js_space, js_trim};
    let s = js_trim(input);
    if s.is_empty() {
        return Err("Paste a link first.".to_string());
    }

    // A wrapped link (`https://tinhat.dev/open#<a server's link>`, share_link.rs) carries the real one in its fragment.
    let s = s.strip_prefix(crate::share_link::OPEN_PREFIX).unwrap_or(s);
    // For both matrix.to and the web client the interesting part lives after `#/`.
    let frag = s.split_once("#/").map(|(_, a)| a).unwrap_or(s);
    // `#/room/…`, or a profile (`#/user/@…`: a person, answered as one)
    let frag = frag.strip_prefix("room/").or_else(|| frag.strip_prefix("user/")).unwrap_or(frag);
    // Percent-encoded forms turn up whenever a link has been through a chat client (and Tin Hat's own links encode
    // the room part like encodeURIComponent: share_link.rs). The web's parseJoinLink decodes the same five.
    let decoded = frag
        .replace("%21", "!")
        .replace("%23", "#")
        .replace("%40", "@")
        .replace("%24", "$")
        .replace("%3A", ":")
        .replace("%3a", ":");

    let (target, query) = decoded.split_once('?').unwrap_or((decoded.as_str(), ""));
    let via: Vec<String> = query
        .split('&')
        .filter_map(|kv| kv.strip_prefix("via="))
        .filter(|v| !v.is_empty())
        .map(crate::share_link::decode_component)
        .collect();
    let target = js_trim(target.trim_end_matches('/'));
    // a link to a message (`!room:server/$event`, matrix.to's and ours): the room. Only a `/` followed by an event id
    // (`$…`) ends the room part (`/` is legal in an alias: a bare `#a/b:server` stays whole). Cut before decoding, so an
    // encoded `%2F` inside an alias stays in it and a broken escape in the event part can't stop the room decoding …
    let sigil = |t: &str| t.starts_with('!') || t.starts_with('#');
    let target = cut_at_event(target);
    // anything still encoded (a non-ASCII alias); a malformed escape keeps the text as it is (the web's decode)
    let target = crate::share_link::decode_component(target);
    let target = js_trim(&target);
    // … and again after it, for a link that encoded the `/$` too
    let target = cut_at_event(target);

    if target.starts_with('@') && target.contains(':') {
        return Ok(JoinTarget::Person(target.to_string()));
    }
    // `!id:server` / `#alias:server` (something on both sides of the `:`), or a room-version-12 id `!opaque` with no
    // server part (the web's parseJoinLink, same two rules)
    let with_server = sigil(target) && target[1..].split_once(':').is_some_and(|(l, r)| !l.is_empty() && !r.is_empty());
    let v12 = target.starts_with('!') && target.len() > 1 && !target.contains(':') && !target.contains(js_space);
    if with_server || v12 {
        return Ok(JoinTarget::RoomOrAlias { id: target.to_string(), via });
    }
    Err("That doesn't look like a Tin Hat link. Paste an invite link, or a room \
         address like #general:yourserver.tinhat.me."
        .to_string())
}

/// The Memories recorder's account — the one the user's server announces (server_offer.rs `recorder`, only while
/// it offers Memories). None = this server has no recorder: then no call tile, alert or receipt treats anyone as
/// one. Its VISIBLE membership + REC indicator are the consent story.
pub(crate) fn recorder_mxid() -> Option<String> {
    crate::server_offer::recorder()
}

pub(crate) async fn do_set_record_call(
    client: &matrix_sdk::Client,
    room_id: &str,
    on: bool,
) -> Result<(), String> {
    let id = matrix_sdk::ruma::RoomId::parse(room_id).map_err(|e| e.to_string())?;
    let room = client.get_room(&id).ok_or_else(|| "room not found".to_string())?;
    if !on {
        // Empty content = off (same shape as clearing a room photo) — leaves the
        // record that recording was once enabled here.
        room.send_state_event_raw(crate::appstate::RECORD_TYPE, "", serde_json::json!({}))
            .await
            .map_err(|e| e.to_string())?;
        return Ok(());
    }
    // Invite the bot first: without it in the room the marker records nothing. An
    // invite that fails because it's already joined/invited is success; a real
    // permission failure is surfaced honestly.
    let Some(recorder) = recorder_mxid() else {
        return Err("this server has no call recorder".to_string());
    };
    let uid = matrix_sdk::ruma::UserId::parse(recorder.as_str()).map_err(|e| e.to_string())?;
    if let Err(e) = invite_without_history(&room, &uid).await {
        let s = e.to_string();
        if !s.contains("already in the room") && !s.contains("already invited") {
            return Err(format!("couldn't invite the recorder: {s}"));
        }
        eprintln!("[vod] invite {recorder}: already present: {s}");
    }
    room.send_state_event_raw(
        crate::appstate::RECORD_TYPE,
        "",
        serde_json::json!({ "active": true, "by": room.own_user_id().to_string() }),
    )
    .await
    .map_err(|e| {
        let s = e.to_string();
        // Member in a room no admin has opened on 0.8.11+ yet: the record marker is
        // still at the room's state_default (50), so Synapse answers M_FORBIDDEN
        // "user_level (0) < send_level (50)". Say what actually unlocks it instead
        // of leaking power-level jargon (field report: "the record button shows but
        // doesn't work for room members").
        if s.contains("M_FORBIDDEN") || s.contains("user_level") {
            "this room isn't unlocked for member recording yet — a room admin just \
needs to open the room once on the latest Tin Hat, then try again"
                .to_string()
        } else {
            s
        }
    })?;
    Ok(())
}

/// Upload an image and publish it as `m.room.avatar`, or clear it when `path` is empty.
///
/// Deliberately the same shape as the personal-avatar path in settings.rs: read the
/// file, guess the mime from the extension, upload, then write the STATE event — which
/// is the only difference that matters, since a room's picture is room state rather
/// than account data.
pub(crate) async fn do_apply_room_photo(
    client: &matrix_sdk::Client,
    room_id: &str,
    path: &str,
) -> Result<(), String> {
    let id = matrix_sdk::ruma::RoomId::parse(room_id).map_err(|e| e.to_string())?;
    let room = client.get_room(&id).ok_or_else(|| "room not found".to_string())?;

    if path.is_empty() {
        // Clearing is an EMPTY CONTENT event, not {"url": ""}: an empty string is not a
        // valid mxc uri, so clients that parse the field strictly reject the event and
        // keep showing the OLD picture. The gate caught exactly that — "Remove photo"
        // reported success while the room kept its avatar. Not a redaction either, which
        // would erase the record of who set what.
        room.send_state_event_raw("m.room.avatar", "", serde_json::json!({}))
            .await
            .map_err(|e| e.to_string())?;
        return Ok(());
    }

    let p = crate::platform::path_from_file_url(path);
    let data = std::fs::read(&p).map_err(|e| format!("read {p}: {e}"))?;
    // docs/PHOTO-METADATA.md: the room photo loses its location + identifying metadata too (can't be cleaned = not set)
    let data = photo_bytes_for_upload("image/*", data).map_err(|_| PHOTO_LOCATION_FAIL.to_string())?;
    let ext = std::path::Path::new(&p)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let mime: mime::Mime = match ext.as_str() {
        "jpg" | "jpeg" => mime::IMAGE_JPEG,
        "gif" => mime::IMAGE_GIF,
        "webp" => "image/webp".parse().unwrap_or(mime::IMAGE_PNG),
        _ => mime::IMAGE_PNG,
    };
    let mxc = client
        .media()
        .upload(&mime, data, None)
        .await
        .map_err(|e| e.to_string())?
        .content_uri;
    room.send_state_event_raw(
        "m.room.avatar",
        "",
        serde_json::json!({ "url": mxc.to_string() }),
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Headless gate for QOL 1.6.1 — proves an audio message received in a room resolves
/// to a real, playable file on disk, which is what the inline player binds to:
///   TINHAT_AUDIO_TEST=1 TINHAT_USER=… TINHAT_PASS=… ./tinhat
///
/// Sends a real audio attachment to a scratch room, reads the event back off the
/// timeline, and runs the SAME resolver the timeline uses. Also checks the prefetch
/// size policy, since that decides whether Play is instant or downloads first.
pub async fn headless_audio_test() -> bool {
    let mut fails = 0;
    let mut check = |label: &str, ok: bool| {
        eprintln!("[audio] {} {label}", if ok { "PASS" } else { "FAIL" });
        if !ok {
            fails += 1;
        }
    };

    // The prefetch policy is pure arithmetic; assert it directly so a future tweak that
    // starts pre-downloading 200 MB files fails here rather than on someone's data plan.
    const MAX: u64 = 8 * 1024 * 1024;
    let prefetch = |size: u64| size == 0 || size <= MAX;
    check("small clip is pre-fetched", prefetch(300_000));
    check("unknown size is pre-fetched", prefetch(0));
    check("oversized file is NOT pre-fetched", !prefetch(60 * 1024 * 1024));
    check("exactly at the limit is pre-fetched", prefetch(MAX));

    let (Ok(user), Ok(pass)) = (std::env::var("TINHAT_USER"), std::env::var("TINHAT_PASS")) else {
        eprintln!("[audio] FAIL: need TINHAT_USER + TINHAT_PASS");
        return false;
    };
    let hs = match crate::bridge::env_homeserver() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("[audio] FAIL: {e}");
            return false;
        }
    };
    let client = match crate::bridge::headless_login(&hs, &user, &pass).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[audio] FAIL: login: {e}");
            return false;
        }
    };
    crate::session::set_client(client.clone());

    use matrix_sdk::ruma::api::client::room::create_room;
    let room = match client.create_room(create_room::v3::Request::new()).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[audio] FAIL: create room: {e}");
            return false;
        }
    };

    // A tiny but REAL ogg/vorbis-shaped payload. The bytes only have to survive the
    // media repo and come back byte-identical; decoding is Qt's job, not this gate's.
    let audio: Vec<u8> = b"OggS\0\x02\0\0\0\0\0\0\0\0tinhat-gate-audio-payload".to_vec();
    let mime: mime::Mime = "audio/ogg".parse().unwrap();
    let sent = room
        .send_attachment("gate.ogg", &mime, audio.clone(), matrix_sdk::attachment::AttachmentConfig::new())
        .await;
    check("audio attachment sent", sent.is_ok());
    let Ok(sent) = sent else {
        eprintln!("[audio] VERDICT: FAIL");
        return false;
    };
    let event_id = sent.event_id;

    // Pull it back the way the timeline does, and confirm the server typed it as audio
    // (the msgtype is what makes the player appear at all).
    let mut content: Option<serde_json::Value> = None;
    for _ in 0..10 {
        if let Ok(ev) = room.event(&event_id, None).await {
            if let Ok(v) = ev.raw().deserialize_as::<serde_json::Value>() {
                content = v.get("content").cloned();
                if content.is_some() {
                    break;
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    let Some(content) = content else {
        check("event read back", false);
        eprintln!("[audio] VERDICT: FAIL");
        return false;
    };
    check(
        "server typed it as m.audio",
        content.get("msgtype").and_then(|m| m.as_str()) == Some("m.audio"),
    );

    // THE point of the feature: the same resolver the timeline uses turns it into a
    // local file the player can open.
    let url = fetch_media(content, event_id.as_str(), Some("audio/ogg"), MediaUse::Play).await.ok();
    check("audio resolved to a local url", url.is_some());
    let path = crate::platform::path_from_file_url(url.as_deref().unwrap_or(""));
    let meta = std::fs::metadata(&path).ok();
    check("resolved file exists", meta.is_some());
    check(
        "resolved file has the bytes we sent",
        meta.map(|m| m.len() == audio.len() as u64).unwrap_or(false),
    );

    // Video takes the same resolver but a different UI path (poster → click → play in
    // place), and unlike audio it is NEVER pre-fetched, so the on-demand resolve is the
    // only thing standing between a click and a black rectangle.
    let video: Vec<u8> = b"\0\0\0\x18ftypmp42tinhat-gate-video-payload".to_vec();
    let vmime: mime::Mime = "video/mp4".parse().unwrap();
    let vsent = room
        .send_attachment("gate.mp4", &vmime, video.clone(), matrix_sdk::attachment::AttachmentConfig::new())
        .await;
    check("video attachment sent", vsent.is_ok());
    if let Ok(vsent) = vsent {
        let vid = vsent.event_id;
        let mut vcontent: Option<serde_json::Value> = None;
        for _ in 0..10 {
            if let Ok(ev) = room.event(&vid, None).await {
                if let Ok(v) = ev.raw().deserialize_as::<serde_json::Value>() {
                    vcontent = v.get("content").cloned();
                    if vcontent.is_some() {
                        break;
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        check(
            "server typed it as m.video",
            vcontent
                .as_ref()
                .and_then(|c| c.get("msgtype"))
                .and_then(|m| m.as_str())
                == Some("m.video"),
        );
        if let Some(vcontent) = vcontent {
            let vurl = fetch_media(vcontent, vid.as_str(), Some("video/mp4"), MediaUse::Play).await.ok();
            check("video resolved to a local url", vurl.is_some());
            let vpath = crate::platform::path_from_file_url(vurl.as_deref().unwrap_or(""));
            let vmeta = std::fs::metadata(&vpath).ok();
            check("resolved video exists", vmeta.is_some());
            check(
                "resolved video has the bytes we sent",
                vmeta.map(|m| m.len() == video.len() as u64).unwrap_or(false),
            );
        }
    }

    let _ = room.leave().await;
    eprintln!("[audio] VERDICT: {}", if fails == 0 { "PASS" } else { "FAIL" });
    fails == 0
}

/// What an attachment is fetched for (security review 2026-10-26: the desktop side of the web's H1; viewer.rs).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum MediaUse {
    /// an m.image decoded in the bubble: only when its BYTES are png / jpeg / gif / webp (never SVG or AVIF), at most
    /// viewer::INLINE_IMAGE_MAX; the cached file carries the sniffed format's extension
    InlineImage,
    /// played in this app (inline audio / video): the viewer list's extension, else `.bin`
    Play,
    /// an audio clip fetched for the timeline without a click (the voice-message player): as Play, but at most
    /// [`AUDIO_PREFETCH_MAX`] bytes downloaded (Astra review 2026-10-26: a forged `info.size` must not pull more)
    PrefetchAudio,
    /// handed to the OS opener: a viewer type only, and only when the bytes are that type
    Open,
    /// shown in the app as plain text (a text-like file, viewer::text_like): `.txt`, bytes that pass the `txt` sniff, at
    /// most viewer::TEXT_VIEW_MAX
    Text,
    /// copied where the user saves it: anything (`.bin` when it is no viewer type)
    Save,
}

/// Why an attachment was not made available (open_media's toast).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum MediaRefused {
    /// not a viewer type (Save as only)
    NotViewable,
    /// the bytes are not what the event says
    Mismatch,
    /// bigger than an inline image may be
    TooBig,
    /// no download (network, keys, no client)
    Failed,
}

impl MediaRefused {
    fn toast(self) -> &'static str {
        match self {
            MediaRefused::NotViewable => "Tin Hat doesn't open this kind of file — use Save as… to keep it",
            MediaRefused::Mismatch => "This file isn't the kind of file it says it is, so Tin Hat won't open it — use Save as… to keep it",
            MediaRefused::TooBig => "That image is too big to show here — use Save as… to keep it",
            MediaRefused::Failed => "Couldn't download that file",
        }
    }
    /// The toast of the in-app text view (launch-gate's web review #8).
    fn text_toast(self) -> &'static str {
        match self {
            MediaRefused::TooBig => "That file is too big to show here — use Save as… to keep it",
            MediaRefused::Mismatch | MediaRefused::NotViewable => {
                "That file isn't plain text, so Tin Hat won't show it — use Save as… to keep it"
            }
            MediaRefused::Failed => "Couldn't download that file",
        }
    }
}

/// At most this much of an audio clip is fetched for the timeline without a click.
pub(crate) const AUDIO_PREFETCH_MAX: u64 = 8 * 1024 * 1024;
/// At most this much of any attachment someone asked for (Open, Save as, Play): far above what a server takes.
pub(crate) const ON_DEMAND_MAX: u64 = 1024 * 1024 * 1024;

/// The most bytes `purpose` may download (Astra review 2026-10-26: enforced while the bytes arrive, not afterwards).
fn media_budget(purpose: MediaUse) -> u64 {
    match purpose {
        MediaUse::InlineImage => crate::viewer::INLINE_IMAGE_MAX,
        MediaUse::PrefetchAudio => AUDIO_PREFETCH_MAX,
        MediaUse::Text => crate::viewer::TEXT_VIEW_MAX,
        MediaUse::Play | MediaUse::Open | MediaUse::Save => ON_DEMAND_MAX,
    }
}

/// The one check an attachment passes before it is handed on, the same for a cached copy and a fresh download (Astra
/// review 2026-10-26: a cached inline image used to skip the size limit): `want` = the extension the purpose asks for
/// (None for an inline image: its bytes decide), `head` = its first bytes, `len` = its full size. Returns the
/// extension the file is (to be) kept under.
pub(crate) fn validate_media(purpose: MediaUse, want: Option<&'static str>, head: &[u8], len: u64) -> Result<&'static str, MediaRefused> {
    use crate::viewer;
    if len > media_budget(purpose) {
        return Err(MediaRefused::TooBig);
    }
    match purpose {
        MediaUse::InlineImage => viewer::sniff_image(head).ok_or(MediaRefused::Mismatch),
        MediaUse::Open => {
            let e = want.ok_or(MediaRefused::NotViewable)?;
            if viewer::sniff(e, head) {
                Ok(e)
            } else {
                Err(MediaRefused::Mismatch)
            }
        }
        MediaUse::Text => {
            if viewer::sniff("txt", head) {
                Ok("txt")
            } else {
                Err(MediaRefused::Mismatch)
            }
        }
        MediaUse::Play | MediaUse::PrefetchAudio | MediaUse::Save => Ok(want.unwrap_or("bin")),
    }
}

/// What Open does with an attachment: hand a viewer type to the OS opener (after its bytes check out), show a text-like
/// file in the app as plain text (launch-gate's web review #8), or nothing — Save as only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenAs {
    Os,
    Text,
    SaveOnly,
}

pub(crate) fn open_as(msgtype: &str, has_media: bool, mime: &str, name: &str) -> OpenAs {
    if !has_media || !matches!(msgtype, "m.file" | "m.image" | "m.video" | "m.audio") {
        return OpenAs::SaveOnly;
    }
    if crate::viewer::open_ext(mime).is_some() {
        OpenAs::Os
    } else if crate::viewer::text_like(mime, name) {
        OpenAs::Text
    } else {
        OpenAs::SaveOnly
    }
}

/// An attachment's file name: `filename` when the event has a caption, else `body`.
fn attachment_name(content: &serde_json::Value) -> String {
    content
        .get("filename")
        .or_else(|| content.get("body"))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

/// Is a 404/405 from the authenticated media endpoint a reason to try the legacy one? Yes unless the server said, in
/// Matrix's own words, that the file doesn't exist (`M_NOT_FOUND`): a server without authenticated media answers
/// `M_UNRECOGNIZED`, and a reverse proxy in front of an older server may answer a plain, non-Matrix 404 (launch-gate's
/// review #12: that used to break every picture on such a server).
fn media_fallback(status: u16, body: &str) -> bool {
    if !matches!(status, 404 | 405) {
        return false;
    }
    let errcode = serde_json::from_str::<serde_json::Value>(body).ok().and_then(|v| v.get("errcode")?.as_str().map(str::to_string));
    errcode.as_deref() != Some("M_NOT_FOUND")
}

/// Download (and decrypt) an attachment holding at most `max` bytes: the transfer stops at `max + 1` (Astra review
/// 2026-10-26: the SDK's get_media_content buffers the whole file before anyone can look at its size, and an event's
/// `info.size` is the sender's word). Authenticated media endpoint with this account's token (the SDK's choice too),
/// the legacy one only when the server doesn't know it; an encrypted file is decrypted (and its hash checked) after,
/// from the at-most-`max` ciphertext. A local echo still in the send queue comes from the SDK's own store.
async fn download_media_capped(source: &matrix_sdk::ruma::events::room::MediaSource, max: u64) -> Result<Vec<u8>, MediaRefused> {
    use matrix_sdk::media::{MediaFormat, MediaRequestParameters};
    use matrix_sdk::ruma::events::room::MediaSource;
    let client = crate::session::client().ok_or(MediaRefused::Failed)?;
    let uri = match source {
        MediaSource::Plain(u) => u.clone(),
        MediaSource::Encrypted(f) => f.url.clone(),
    };
    let (server, media_id) = uri.parts().map_err(|_| MediaRefused::Failed)?;
    if server.as_str() == "send-queue.localhost" {
        let req = MediaRequestParameters { source: source.clone(), format: MediaFormat::File };
        let b = client.media().get_media_content(&req, true).await.map_err(|_| MediaRefused::Failed)?;
        return if b.len() as u64 > max { Err(MediaRefused::TooBig) } else { Ok(b) };
    }
    let token = client.access_token().ok_or(MediaRefused::Failed)?;
    let endpoint = |path: &[&str]| -> Result<url::Url, MediaRefused> {
        let mut u = client.homeserver();
        u.set_query(None);
        u.set_fragment(None);
        u.path_segments_mut().map_err(|_| MediaRefused::Failed)?.pop_if_empty().extend(path).push(server.as_str()).push(media_id);
        Ok(u)
    };
    let http = crate::proxy::http(None);
    let mut resp = http
        .get(endpoint(&["_matrix", "client", "v1", "media", "download"])?)
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|_| MediaRefused::Failed)?;
    if matches!(resp.status().as_u16(), 404 | 405) {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        if !media_fallback(status, &body) {
            return Err(MediaRefused::Failed);
        }
        resp = http
            .get(endpoint(&["_matrix", "media", "v3", "download"])?)
            .send()
            .await
            .map_err(|_| MediaRefused::Failed)?;
    }
    if !resp.status().is_success() {
        return Err(MediaRefused::Failed);
    }
    if resp.content_length().is_some_and(|n| n > max) {
        return Err(MediaRefused::TooBig);
    }
    let mut data: Vec<u8> = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|_| MediaRefused::Failed)? {
        if data.len() as u64 + chunk.len() as u64 > max {
            return Err(MediaRefused::TooBig);
        }
        data.extend_from_slice(&chunk);
    }
    match source {
        MediaSource::Plain(_) => Ok(data),
        MediaSource::Encrypted(file) => {
            use std::io::Read;
            let mut cursor = std::io::Cursor::new(data);
            let reader = matrix_sdk_base::crypto::AttachmentDecryptor::new(&mut cursor, file.as_ref().clone().into())
                .map_err(|_| MediaRefused::Failed)?;
            let mut out = Vec::new();
            reader.take(max + 1).read_to_end(&mut out).map_err(|_| MediaRefused::Failed)?;
            if out.len() as u64 > max {
                return Err(MediaRefused::TooBig);
            }
            Ok(out)
        }
    }
}

/// Download + decrypt an attachment into the media cache (`<event>.<ext>`, the extension from viewer.rs, never from
/// the event), returning a `file://` url. Cached by event id, so after the first load this is a `Path::exists` check
/// plus [`validate_media`] (the same check a fresh download passes). `purpose` decides what may come back and how much
/// may be downloaded (see [`MediaUse`]).
pub(crate) async fn fetch_media(content: serde_json::Value, event_id: &str, mime: Option<&str>, purpose: MediaUse) -> Result<String, MediaRefused> {
    use crate::viewer;
    use matrix_sdk::ruma::events::room::message::MessageType;

    let dir = media_cache_dir();
    let id = sanitize_id(event_id);
    let mime = mime.unwrap_or("");
    let want: Option<&'static str> = match purpose {
        MediaUse::InlineImage => None, // decided by the bytes
        MediaUse::Open => Some(viewer::open_ext(mime).ok_or(MediaRefused::NotViewable)?),
        MediaUse::Text => Some("txt"),
        MediaUse::Play | MediaUse::PrefetchAudio | MediaUse::Save => Some(viewer::viewer_ext(mime).unwrap_or("bin")),
    };
    let candidates: &[&str] = match want {
        Some(ref e) => std::slice::from_ref(e),
        None => viewer::INLINE_EXTS,
    };
    for ext in candidates {
        let path = dir.join(format!("{id}.{ext}"));
        if path.exists() {
            // a copy cached for playing or saving was never checked: it passes the same check as a fresh download
            // before the OS or Qt's image plugins get it (an unchecked `.png` that isn't one would be
            // content-sniffed — an SVG among them; one bigger than an inline image may be is not decoded inline)
            let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(u64::MAX);
            let ok = validate_media(purpose, want, &viewer::file_head(&path, 8192), len)?;
            if ok != *ext {
                return Err(MediaRefused::Mismatch);
            }
            touch_mtime(&path); // keep viewed media warm for the LRU cache sweep
            return Ok(crate::platform::file_url(&path));
        }
    }
    let mt: MessageType = serde_json::from_value(content).map_err(|_| MediaRefused::Failed)?;
    // Any media-bearing message type resolves the same way (download + decrypt to the
    // LRU cache): images inline at load; video/file/audio on demand via open_media
    // ("no clear way to view an mp4" — 0.3.3 feedback).
    let source = match mt {
        MessageType::Image(m) => m.source,
        MessageType::Video(m) => m.source,
        MessageType::File(m) => m.source,
        MessageType::Audio(m) => m.source,
        _ => return Err(MediaRefused::Failed),
    };
    let bytes = download_media_capped(&source, media_budget(purpose)).await?;
    let ext = validate_media(purpose, want, &bytes, bytes.len() as u64)?;
    let path = dir.join(format!("{id}.{ext}"));
    tokio::fs::create_dir_all(&dir).await.map_err(|_| MediaRefused::Failed)?;
    tokio::fs::write(&path, &bytes).await.map_err(|_| MediaRefused::Failed)?;
    Ok(crate::platform::file_url(&path))
}

/// LRU touch: bump a cache file's mtime so the startup sweep treats recently-viewed
/// entries as warm. Failures are ignored — a failed touch must never turn a cache hit
/// into a miss.
pub(crate) fn touch_mtime(path: &std::path::Path) {
    if let Ok(f) = std::fs::File::options().append(true).open(path) {
        let _ = f.set_modified(std::time::SystemTime::now());
    }
}

/// Kick off the once-per-launch background sweep of the unbounded on-disk caches:
/// decrypted inline media (`media_cache_dir`) and avatars (`bridge::data_dir()/avatars`).
/// LRU by mtime (fetch_media / bridge::resolve_avatar touch on hit) with
/// generous TTL + size budgets so files referenced by the instant-paint timeline caches
/// are rarely evicted — a stale reference self-heals via re-download on the next load.
pub(crate) fn spawn_cache_sweep() {
    let _ = crate::bridge::runtime().spawn_blocking(|| {
        // security review 2026-10-26: `<id>.img` (content-sniffed by Qt, an SVG among them) is no longer written; the
        // copies older versions left go (they are fetched again, as what their bytes are — viewer.rs)
        for dir in [media_cache_dir(), crate::linkpreview::cache_dir(), crate::bridge::data_dir().join("avatars"), crate::settings::avatar_cache_dir()] {
            drop_legacy_img(&dir);
        }
        sweep_cache_dir(&media_cache_dir(), 512 * 1024 * 1024, 60);
        sweep_cache_dir(&crate::linkpreview::cache_dir(), 64 * 1024 * 1024, 14);
        sweep_cache_dir(&crate::bridge::data_dir().join("avatars"), 64 * 1024 * 1024, 90);
    });
}

/// Delete the plain `*.img` files directly in a cache dir (no recursion, symlinks untouched).
fn drop_legacy_img(dir: &std::path::Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x == "img") && std::fs::symlink_metadata(&p).is_ok_and(|m| m.is_file()) {
            let _ = std::fs::remove_file(&p);
        }
    }
}

/// Delete regular files from a cache dir, oldest-mtime-first, until it fits
/// `budget_bytes`; anything older than `ttl_days` goes regardless. Only plain files
/// directly inside `dir` are touched — no recursion, and symlinks are never followed
/// (symlink_metadata reports the link itself, which fails `is_file`).
fn sweep_cache_dir(dir: &std::path::Path, budget_bytes: u64, ttl_days: u64) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let now = std::time::SystemTime::now();
    let ttl = std::time::Duration::from_secs(ttl_days * 24 * 60 * 60);
    let mut files: Vec<(std::time::SystemTime, u64, std::path::PathBuf)> = Vec::new();
    for entry in rd.flatten() {
        let path = entry.path();
        let Ok(md) = std::fs::symlink_metadata(&path) else { continue };
        if !md.is_file() {
            continue;
        }
        files.push((md.modified().unwrap_or(now), md.len(), path));
    }
    let mut total: u64 = files.iter().map(|(_, size, _)| *size).sum();
    files.sort_by_key(|(mtime, _, _)| *mtime); // oldest first
    let mut removed = 0usize;
    for (mtime, size, path) in &files {
        let expired = now.duration_since(*mtime).map_or(false, |age| age > ttl);
        if total <= budget_bytes && !expired {
            break; // sorted oldest-first: the rest are younger and we're under budget
        }
        if std::fs::remove_file(path).is_ok() {
            total = total.saturating_sub(*size);
            removed += 1;
        }
    }
    if removed > 0 {
        eprintln!(
            "[cache] swept {removed} file(s) from {} ({total} bytes kept)",
            dir.display()
        );
    }
}

/// Resolve a sender's friendly display name from the room's member cache (no network);
/// falls back to the mxid localpart. Used to label timeline bubbles.
/// 10.1 (TH-TL-09): one pinned event that is not in the loaded window, as a popover row.
/// Fetched once per event id per session (pins change rarely; forget_session at sign-out).
async fn pinned_row_fetch(room: &Room, event_id: &str) -> Option<serde_json::Value> {
    let cache = pinned_rows();
    if let Some(v) = cache.lock().ok().and_then(|m| m.get(event_id).cloned()) {
        return Some(v);
    }
    let eid = matrix_sdk::ruma::EventId::parse(event_id).ok()?;
    let ev = room.event(&eid, None).await.ok()?;
    let v = ev.raw().deserialize_as::<serde_json::Value>().ok()?;
    let sender = v.get("sender").and_then(|s| s.as_str()).unwrap_or("").to_string();
    let raw_body = v.get("content").and_then(|c| c.get("body")).and_then(|b| b.as_str()).unwrap_or("");
    // QA W1: deleted before encrypted (a redacted encrypted event keeps its type and loses its content)
    let deleted = v.get("unsigned").and_then(|u| u.get("redacted_because")).is_some();
    let encrypted = raw_body.is_empty() && v.get("type").and_then(|t| t.as_str()) == Some("m.room.encrypted");
    let body = crate::pins::pin_row_body(encrypted, deleted, &crate::embeds::strip_reply_fallback(raw_body));
    let ts = v.get("origin_server_ts").and_then(|t| t.as_u64()).unwrap_or(0);
    let name = resolve_member_name(room, &sender).await;
    // widgets W1: the sender's avatar and the shared snippet (the event as decrypted here; still encrypted = no key)
    let avatar = sender_avatar(room, &sender, &mut std::collections::HashMap::new()).await;
    let sn = tinhat_widgets::recent::snippet_of(&serde_json::json!({
        "type": v.get("type").cloned().unwrap_or_default(),
        "content": v.get("content").cloned().unwrap_or_default(),
        "redacted": deleted,
    }));
    let row = serde_json::json!({
        "id": event_id, "senderName": name, "body": body, "ts": ts, "deleted": deleted,
        "sender": sender, "avatar": avatar, "kind": sn.kind, "text": sn.text,
    });
    if let Ok(mut m) = cache.lock() {
        m.insert(event_id.to_string(), row.clone());
    }
    Some(row)
}

pub(crate) async fn resolve_member_name(room: &Room, mxid: &str) -> String {
    if let Ok(uid) = matrix_sdk::ruma::UserId::parse(mxid) {
        if let Ok(Some(m)) = room.get_member_no_sync(&uid).await {
            if let Some(dn) = m.display_name() {
                if !dn.is_empty() {
                    return dn.to_string();
                }
            }
        }
    }
    mxid.trim_start_matches('@').split(':').next().unwrap_or(mxid).to_string()
}

#[cfg(test)]
mod attachment_retry_tests {
    // fix/attach-retry-txn (12.1.15 merge): the attachment now goes out through attach-encrypt-recheck's send_raw, so the
    // echo's transaction id has to ride on THAT request: a Retry after a lost response repeats the same transaction and
    // the server answers with the event it already has, instead of posting the file a second time.
    #[test]
    fn a_retried_attachment_repeats_the_echos_transaction() {
        let src = include_str!("room.rs");
        // perform_echo's own body only (astra: the whole rest of the file also holds this test's own strings)
        let f = src.find("async fn perform_echo(").expect("perform_echo");
        let body = &src[f..f + src[f..].find("\n}\n").expect("the end of perform_echo")];
        assert!(!body.contains("mod attachment_retry_tests"), "the search must stop at perform_echo's end");
        assert!(body.contains("let txn: OwnedTransactionId = e.txn.as_str().into();"), "the txn is the echo's own");
        let start = body.find("EchoKind::Attachment(path) => {").expect("the attachment arm");
        let end = start + body[start..].find("EchoKind::Voice(v) => {").expect("the end of the arm");
        let arm = &body[start..end];
        assert!(arm.contains(".send_raw(\"m.room.message\", content)"), "the event goes out with send_raw");
        assert!(arm.contains(".with_transaction_id(&txn)"), "a Retry has to be the same transaction");
    }
}

#[cfg(test)]
mod reply_mention_tests {
    use super::reply_mention_ids;

    /// REACH A5.2 — the web mirrors this name (chat/actions tests).
    #[test]
    fn reply_mentions() {
        let me = "@me:example.com";
        let simon = "@simon_r:example.com";
        let dale = "@dale:example.com";
        // the parent's sender is added
        assert_eq!(reply_mention_ids(Some(simon), me, &[]), vec![simon]);
        // you are never added — replying to yourself, or picking yourself
        assert!(reply_mention_ids(Some(me), me, &[]).is_empty());
        assert_eq!(reply_mention_ids(Some(simon), me, &[me.to_string()]), vec![simon]);
        // picked mentions are kept (as typed, first), and nothing appears twice
        assert_eq!(reply_mention_ids(Some(simon), me, &[dale.to_string(), simon.to_string()]), vec![dale, simon]);
        assert_eq!(reply_mention_ids(None, me, &[dale.to_string(), dale.to_string()]), vec![dale]);
        // an unknown parent (not loaded, failed lookup) still keeps the picks
        assert_eq!(reply_mention_ids(None, me, &[dale.to_string()]), vec![dale]);
        // the parent's sender comes from the loaded row (no lookup needed)
        let rows = r#"[{"id":"$a","sender":"@simon_r:example.com"},{"id":"~txn1","sender":"@me:example.com","echo":true}]"#;
        assert_eq!(super::row_sender(rows, "$a"), simon);
        assert_eq!(super::row_sender(rows, "$missing"), "");
        assert_eq!(super::row_sender("", "$a"), "");
    }
}

#[cfg(test)]
mod media_fallback_tests {
    use super::media_fallback;

    /// Launch-gate's review #12: the legacy media endpoint is tried on any 404/405 except Matrix's own "no such file".
    #[test]
    fn a_plain_404_falls_back_and_a_missing_file_does_not() {
        // a server without authenticated media
        assert!(media_fallback(404, r#"{"errcode":"M_UNRECOGNIZED","error":"Unrecognized request"}"#));
        assert!(media_fallback(405, r#"{"errcode":"M_UNRECOGNIZED"}"#));
        // a reverse proxy's own 404 page, or nothing at all
        assert!(media_fallback(404, "<html><body>404 Not Found</body></html>"));
        assert!(media_fallback(404, ""));
        assert!(media_fallback(405, "Method Not Allowed"));
        // the file really is missing: no second request
        assert!(!media_fallback(404, r#"{"errcode":"M_NOT_FOUND","error":"Not found"}"#));
        // other statuses never fall back (a 401/403 is about the token, a 5xx about the server)
        for st in [200u16, 401, 403, 429, 500, 502] {
            assert!(!media_fallback(st, r#"{"errcode":"M_UNRECOGNIZED"}"#), "{st}");
        }
    }
}

#[cfg(test)]
mod tests {

    use super::{parse_join_link, JoinTarget};

    fn room(id: &str, via: &[&str]) -> JoinTarget {
        JoinTarget::RoomOrAlias { id: id.to_string(), via: via.iter().map(|v| v.to_string()).collect() }
    }

    #[test]
    fn parses_a_matrix_to_link() {
        assert_eq!(
            parse_join_link("https://matrix.to/#/!abc:example.com?via=example.com").unwrap(),
            room("!abc:example.com", &["example.com"])
        );
    }

    #[test]
    fn parses_a_web_client_link() {
        assert_eq!(
            parse_join_link("https://element.example.com/#/room/#general:example.com").unwrap(),
            room("#general:example.com", &[])
        );
        // 10.5.1: the Tin Hat web app's links (tinhat.example.com) parse the same way
        assert_eq!(
            parse_join_link("https://tinhat.example.com/#/room/#general:example.com").unwrap(),
            room("#general:example.com", &[])
        );
        assert_eq!(
            parse_join_link("https://tinhat.example.com/#/room/!abc:example.com").unwrap(),
            room("!abc:example.com", &[])
        );
    }

    #[test]
    fn parses_a_bare_alias_or_id() {
        assert_eq!(parse_join_link("#general:example.com").unwrap(), room("#general:example.com", &[]));
        assert_eq!(parse_join_link("  !abc:example.com  ").unwrap(), room("!abc:example.com", &[]));
    }

    #[test]
    fn parses_percent_encoded_links() {
        // What you get when a link has been through a chat client.
        assert_eq!(
            parse_join_link("https://matrix.to/#/%23general%3Aexample.com").unwrap(),
            room("#general:example.com", &[])
        );
    }

    #[test]
    fn a_user_id_is_reported_as_a_person() {
        assert_eq!(
            parse_join_link("https://matrix.to/#/@blake:example.com").unwrap(),
            JoinTarget::Person("@blake:example.com".to_string())
        );
    }

    #[test]
    fn rubbish_is_refused_with_an_explanation() {
        assert!(parse_join_link("").is_err());
        assert!(parse_join_link("hello").is_err());
        assert!(parse_join_link("https://example.com/nope").is_err());
        // A sigil with no server is not a room address.
        assert!(parse_join_link("#general").is_err());
    }

    use super::{html_escape, render_mention_html};

    #[test]
    fn mention_html_basic() {
        let m = vec![("Alice".to_string(), "@alice:example.com".to_string())];
        let html = render_mention_html("hey @Alice ping", &m);
        assert_eq!(html, "hey <a href=\"https://matrix.to/#/@alice:example.com\">@Alice</a> ping");
    }

    #[test]
    fn mention_html_escapes_and_newlines() {
        // Body is HTML-escaped; newlines become <br/>; a non-mention "@" is left alone.
        let m = vec![("Bob".to_string(), "@bob:example.com".to_string())];
        let html = render_mention_html("look <b> & @Bob\nbye @notamember", &m);
        assert_eq!(
            html,
            "look &lt;b&gt; &amp; <a href=\"https://matrix.to/#/@bob:example.com\">@Bob</a><br/>bye @notamember"
        );
    }

    #[test]
    fn mention_html_repeated_name_not_double_wrapped() {
        // Both occurrences link; the anchor's own "@Cat" text is NOT re-matched.
        let m = vec![("Cat".to_string(), "@cat:example.com".to_string())];
        let html = render_mention_html("@Cat and @Cat", &m);
        assert_eq!(html.matches("<a href=").count(), 2);
        assert!(!html.contains("@Cat</a></a>"));
    }

    #[test]
    fn escape_only_specials() {
        assert_eq!(html_escape("a<b>&\"c"), "a&lt;b&gt;&amp;&quot;c");
    }

    // ---- 10.5.2 (timeline): "going back into a room, the conversation does not load" ----------

    use super::{
        backup_download_wanted, note_backup_downloaded, read_snapshot_at, retry_backoff, same_open,
        sync_window, write_snapshot_atomic, TimelineWindow, WindowPage, WindowPass, SCAN_BUDGET,
    };
    use serde_json::{json, Value};

    /// A room's filtered /messages timeline, oldest first. Tokens are positions counted from the
    /// start of the room, so they stay valid while new events arrive (like real pagination tokens).
    struct FakeRoom {
        events: Vec<Value>,
    }

    impl FakeRoom {
        fn new() -> Self {
            Self { events: Vec::new() }
        }
        fn message(&mut self) {
            let n = self.events.len();
            self.events.push(json!({
                "type": "m.room.message", "event_id": format!("$m{n}"), "sender": "@simon_r:example.com",
                "origin_server_ts": n, "content": { "msgtype": "m.text", "body": format!("message {n}") }
            }));
        }
        /// What an Element Call media-key event looks like once decrypted: on the wire it is an
        /// `m.room.encrypted`, so the server-side type filter lets it through; it is not a row.
        fn call_key(&mut self) {
            let n = self.events.len();
            self.events.push(json!({
                "type": "io.element.call.encryption_keys", "event_id": format!("$k{n}"),
                "sender": "@wren:example.com", "origin_server_ts": n, "content": { "keys": [] }
            }));
        }
        fn page(&self, from: Option<String>, limit: u16) -> WindowPage {
            let end = from.map(|f| f.parse::<usize>().expect("token")).unwrap_or(self.events.len());
            let start = end.saturating_sub(limit as usize);
            WindowPage {
                chunk: self.events[start..end].iter().rev().cloned().collect(),
                end: if start > 0 { Some(start.to_string()) } else { None },
            }
        }
    }

    /// One window pass against the fake room (its pages are always ready, so no runtime is needed).
    fn pass(win: &mut TimelineWindow, want: usize, room: &FakeRoom) -> WindowPass {
        use futures_util::FutureExt;
        sync_window(win, want, |from, limit| std::future::ready(Ok(room.page(from, limit))))
            .now_or_never()
            .expect("the fake room never waits")
            .expect("the fake room never fails")
    }

    fn newest_row_id(win: &TimelineWindow) -> String {
        win.events
            .iter()
            .find(|v| super::is_rendered_row(v))
            .and_then(|v| v["event_id"].as_str())
            .unwrap_or("")
            .to_string()
    }

    #[test]
    fn going_back_after_a_call_key_flood_still_shows_the_conversation() {
        // A field report, desktop half: a room whose window is warm, then a call night's worth of
        // media-key events (more than one warm pass reaches: 24 pages x 30) while you are elsewhere.
        let mut room = FakeRoom::new();
        for _ in 0..120 {
            room.message();
        }
        let mut win = TimelineWindow::new();
        let first = pass(&mut win, 50, &room);
        assert!(!first.gap && win.msg_rows >= 50);
        for _ in 0..1000 {
            room.call_key();
        }
        let back = pass(&mut win, 50, &room);
        assert!(back.gap, "the frontier is out of one warm pass's reach: {back:?}");
        // Before 10.5.2 the rebuilt window held only the 720 newest events — all call keys — and the
        // tail had no budget left: 0 rows, drawn as "No messages".
        assert!(win.msg_rows >= 50, "went back into the room and got {} rows ({back:?})", win.msg_rows);
        assert_eq!(newest_row_id(&win), "$m119", "the newest message is on screen");
        assert!(back.scanned <= 720 + SCAN_BUDGET, "still bounded: {back:?}");
    }

    #[test]
    fn going_back_after_a_few_messages_merges_at_the_frontier() {
        let mut room = FakeRoom::new();
        for _ in 0..120 {
            room.message();
        }
        let mut win = TimelineWindow::new();
        pass(&mut win, 50, &room);
        let rows = win.msg_rows;
        for _ in 0..5 {
            room.message();
        }
        room.call_key();
        let back = pass(&mut win, 50, &room);
        assert!(back.hit_frontier && !back.gap, "{back:?}");
        assert_eq!(back.pages, 1, "one warm page, not a rewalk: {back:?}");
        assert_eq!(win.msg_rows, rows + 5);
        assert_eq!(newest_row_id(&win), "$m124");
        // No duplicates after the merge.
        let mut ids: Vec<&str> = win.events.iter().filter_map(|v| v["event_id"].as_str()).collect();
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n);
    }

    #[test]
    fn a_gap_refill_stays_bounded_in_a_room_that_is_all_call_keys() {
        let mut room = FakeRoom::new();
        for _ in 0..60 {
            room.message();
        }
        let mut win = TimelineWindow::new();
        pass(&mut win, 50, &room);
        for _ in 0..20_000 {
            room.call_key();
        }
        let back = pass(&mut win, 50, &room);
        assert!(back.gap);
        assert!(back.scanned <= 720 + SCAN_BUDGET, "scan bound kept: {back:?}");
        assert!(win.from_token.is_some(), "more history is still reachable (the view keeps paging)");
    }

    #[test]
    fn a_cold_open_of_a_flooded_room_reaches_the_messages_within_the_view_fill_passes() {
        // The cold budget (900) can come back with 0 rows; ConversationView then pages on its own
        // (_fillIfShort: up to 8 load_older passes, +50 rows each). Those passes must get there.
        let mut room = FakeRoom::new();
        for _ in 0..40 {
            room.message();
        }
        for _ in 0..5_000 {
            room.call_key();
        }
        let mut win = TimelineWindow::new();
        let first = pass(&mut win, 50, &room);
        assert!(first.scanned <= 2 * SCAN_BUDGET, "{first:?}");
        assert_eq!(win.msg_rows, 0);
        let mut want = 50;
        for _ in 0..8 {
            if win.msg_rows > 0 || win.from_token.is_none() {
                break;
            }
            want += 50;
            let p = pass(&mut win, want, &room);
            assert!(p.scanned <= SCAN_BUDGET + 30, "each fill pass is budgeted: {p:?}");
        }
        assert!(win.msg_rows > 0, "the messages under 5,000 call keys were reached");
    }

    #[test]
    fn a_cold_open_under_1500_non_message_events_paints_the_messages_at_once() {
        // What the switch gate hit on a fresh start: the room's newest 900 events were all
        // reactions, the first pass found 0 rows and the view said "No messages" until the next page.
        let mut room = FakeRoom::new();
        for _ in 0..88 {
            room.message();
        }
        for _ in 0..1500 {
            room.call_key();
        }
        let mut win = TimelineWindow::new();
        let first = pass(&mut win, 50, &room);
        assert!(win.msg_rows >= 50, "first paint had {} rows ({first:?})", win.msg_rows);
        assert!(first.scanned <= 2 * SCAN_BUDGET, "{first:?}");
    }

    #[test]
    fn a_result_from_an_earlier_visit_of_the_same_room_is_dropped() {
        // open A (gen 1) starts a load; open B (gen 2); open A again (gen 3); the gen-1 load lands.
        // The room-id stamp alone accepted it (same room) — it cleared `busy` early and could paint
        // older data over newer.
        assert!(!same_open(3, "!a:example.com", 1, "!a:example.com"));
        assert!(same_open(3, "!a:example.com", 3, "!a:example.com"));
        assert!(!same_open(3, "!a:example.com", 3, "!b:example.com"));
    }

    #[test]
    fn a_failed_load_retries_on_its_own() {
        let secs: Vec<u64> = (1..=7).map(|n| retry_backoff(n).as_secs()).collect();
        assert_eq!(secs, vec![2, 4, 8, 16, 30, 30, 30]);
    }

    #[test]
    fn the_key_backup_is_pulled_once_per_room_unless_messages_are_unreadable() {
        let room = "!backup-once-test:example.com";
        assert!(backup_download_wanted(room, false));
        note_backup_downloaded(room);
        assert!(!backup_download_wanted(room, false), "not again on every open");
        assert!(backup_download_wanted(room, true), "again while the window holds unreadable events");
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("tinhat-timeline-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_torn_snapshot_is_never_painted() {
        let d = scratch("torn");
        let p = d.join("room.json");
        std::fs::write(&p, "[{\"id\":\"$a\",\"body\":\"hi\"},{\"id\":\"$b\",\"bo").unwrap();
        assert!(read_snapshot_at(&p).is_none(), "a half-written snapshot must not reach QML");
        assert!(!p.exists(), "and it is removed so the next open doesn't trip on it");
        write_snapshot_atomic(&p, "[{\"id\":\"$a\"},{\"id\":\"$b\"}]").unwrap();
        assert_eq!(read_snapshot_at(&p).map(|s| s.1), Some(2));
        std::fs::write(&p, "  ").unwrap();
        assert!(read_snapshot_at(&p).is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn concurrent_snapshot_writes_never_leave_a_torn_file() {
        // The main view and a floating chat of the same room reload on the same sync and both write
        // its snapshot; the next open reads it. Every read must see one whole snapshot.
        let d = scratch("concurrent");
        let p = std::sync::Arc::new(d.join("room.json"));
        write_snapshot_atomic(&p, "[]").unwrap();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reader = {
            let (p, stop) = (p.clone(), stop.clone());
            std::thread::spawn(move || {
                let (mut reads, mut bad) = (0u32, 0u32);
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    if let Ok(s) = std::fs::read_to_string(&*p) {
                        reads += 1;
                        if serde_json::from_str::<Vec<Value>>(&s).is_err() {
                            bad += 1;
                        }
                    }
                }
                (reads, bad)
            })
        };
        let writers: Vec<_> = (0..6)
            .map(|w| {
                let p = p.clone();
                std::thread::spawn(move || {
                    for i in 0..150 {
                        let rows: Vec<Value> = (0..(20 + (w * 37 + i * 11) % 180))
                            .map(|n| json!({ "id": format!("$e{w}-{i}-{n}"), "body": "x".repeat(40 + n % 90) }))
                            .collect();
                        write_snapshot_atomic(&p, &serde_json::to_string(&rows).unwrap()).unwrap();
                    }
                })
            })
            .collect();
        for w in writers {
            w.join().unwrap();
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let (reads, bad) = reader.join().unwrap();
        assert!(reads > 0);
        assert_eq!(bad, 0, "{bad} of {reads} reads saw a torn snapshot");
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// THEMES-UI.md §14.2: a message's theme share card as the row carries it — `null` when the content has no
/// `dev.tinhat.theme.share` (the message as usual), else theme_editor.rs `share_row` (the contract's reader + the
/// card's colours, computed by this receiver), cached per event id (bounded).
fn theme_share_of(event_id: &str, content: &serde_json::Value) -> serde_json::Value {
    if content.get(tinhat_theme::share::SHARE_KEY).is_none() {
        return serde_json::Value::Null;
    }
    let cache = theme_shares();
    if let Some(v) = cache.lock().unwrap_or_else(|p| p.into_inner()).get(event_id) {
        return v.clone();
    }
    let v = crate::theme_editor::share_row(content);
    let mut c = cache.lock().unwrap_or_else(|p| p.into_inner());
    if c.len() > 512 {
        c.clear();
    }
    c.insert(event_id.to_string(), v.clone());
    v
}

#[cfg(test)]
mod attach_encrypt_tests {
    use super::{attach_upload_checked, attach_upload_encrypt, attachment_content};
    use matrix_sdk::ruma::events::room::MediaSource;
    use matrix_sdk::ruma::OwnedMxcUri;
    use matrix_sdk::EncryptionState::{self, Encrypted, NotEncrypted, Unknown};
    use std::cell::RefCell;

    fn dup(s: &EncryptionState) -> EncryptionState {
        match s {
            Encrypted => Encrypted,
            NotEncrypted => NotEncrypted,
            _ => Unknown,
        }
    }

    fn block<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(f)
    }

    /// A room whose state changes on a script: each `latest` / `current` read takes the next value of its list (the last
    /// repeats), each upload records `encrypted` and applies `after_upload`.
    struct Script {
        state: RefCell<EncryptionState>,
        after_upload: RefCell<Vec<EncryptionState>>,
        uploads: RefCell<Vec<bool>>,
    }
    impl Script {
        fn new(start: EncryptionState, after_upload: Vec<EncryptionState>) -> Self {
            Self { state: RefCell::new(start), after_upload: RefCell::new(after_upload), uploads: RefCell::new(vec![]) }
        }
        fn run(&self) -> Result<bool, String> {
            block(attach_upload_checked(
                || async { Ok(dup(&self.state.borrow())) },
                || dup(&self.state.borrow()),
                |encrypted| async move {
                    self.uploads.borrow_mut().push(encrypted);
                    let mut next = self.after_upload.borrow_mut();
                    if !next.is_empty() {
                        *self.state.borrow_mut() = next.remove(0);
                    }
                    Ok(encrypted)
                },
            ))
        }
    }

    #[test]
    fn the_decision_table() {
        assert_eq!(attach_upload_encrypt(&NotEncrypted, &NotEncrypted), Ok(false));
        assert_eq!(attach_upload_encrypt(&Encrypted, &NotEncrypted), Ok(true));
        assert_eq!(attach_upload_encrypt(&NotEncrypted, &Encrypted), Ok(true));
        assert!(attach_upload_encrypt(&Unknown, &NotEncrypted).is_err());
        assert!(attach_upload_encrypt(&NotEncrypted, &Unknown).is_err());
    }

    #[test]
    fn a_plain_room_uploads_once_in_the_clear() {
        let s = Script::new(NotEncrypted, vec![]);
        assert_eq!(s.run(), Ok(false));
        assert_eq!(*s.uploads.borrow(), vec![false]);
    }

    #[test]
    fn an_encrypted_room_uploads_once_encrypted() {
        let s = Script::new(Encrypted, vec![]);
        assert_eq!(s.run(), Ok(true));
        assert_eq!(*s.uploads.borrow(), vec![true]);
    }

    #[test]
    fn encryption_turned_on_during_the_upload_uploads_again_encrypted() {
        let s = Script::new(NotEncrypted, vec![Encrypted]);
        assert_eq!(s.run(), Ok(true), "the media handed to the event must be the encrypted upload");
        assert_eq!(*s.uploads.borrow(), vec![false, true]);
    }

    /// The synchronous read right before the upload counts, even when the first read said plain.
    #[test]
    fn the_reread_right_before_the_upload_counts() {
        let uploads = RefCell::new(vec![]);
        let r: Result<bool, String> = block(attach_upload_checked(
            || async { Ok(NotEncrypted) },
            || Encrypted,
            |e| {
                uploads.borrow_mut().push(e);
                async move { Ok(e) }
            },
        ));
        assert_eq!(r, Ok(true));
        assert_eq!(*uploads.borrow(), vec![true]);
    }

    #[test]
    fn unknown_at_the_reread_uploads_nothing() {
        let uploads = RefCell::new(0);
        let r: Result<bool, String> = block(attach_upload_checked(
            || async { Ok(NotEncrypted) },
            || Unknown,
            |e| {
                *uploads.borrow_mut() += 1;
                async move { Ok(e) }
            },
        ));
        assert!(r.is_err());
        assert_eq!(*uploads.borrow(), 0);
    }

    #[test]
    fn a_failed_state_read_uploads_nothing() {
        let uploads = RefCell::new(0);
        let r: Result<bool, String> = block(attach_upload_checked(
            || async { Err("no answer".to_string()) },
            || NotEncrypted,
            |e| {
                *uploads.borrow_mut() += 1;
                async move { Ok(e) }
            },
        ));
        assert!(r.is_err());
        assert_eq!(*uploads.borrow(), 0);
    }

    #[test]
    fn unknown_uploads_nothing() {
        let s = Script::new(Unknown, vec![]);
        assert!(s.run().is_err());
        assert!(s.uploads.borrow().is_empty());
    }

    #[test]
    fn unknown_after_a_plain_upload_sends_nothing() {
        let s = Script::new(NotEncrypted, vec![Unknown]);
        assert!(s.run().is_err());
        assert_eq!(*s.uploads.borrow(), vec![false]);
    }

    #[test]
    fn a_state_that_never_settles_gives_up() {
        let s = Script::new(NotEncrypted, vec![Encrypted, NotEncrypted, Encrypted, NotEncrypted]);
        // an encrypted upload ends it as soon as one happens; only flip-flopping reads past three tries can fail
        assert_eq!(s.run(), Ok(true));
        let always_flips = RefCell::new(0);
        let r: Result<bool, String> = block(attach_upload_checked(
            || async {
                let mut n = always_flips.borrow_mut();
                *n += 1;
                Ok(if *n % 2 == 1 { NotEncrypted } else { Encrypted })
            },
            || NotEncrypted,
            |e| async move { Ok(e) },
        ));
        assert!(r.is_err());
    }

    fn plain() -> MediaSource {
        MediaSource::Plain(OwnedMxcUri::from("mxc://example.org/abc"))
    }

    #[test]
    fn content_matches_what_the_sdk_built() {
        let c = |m: &str, w, h, size| serde_json::to_value(attachment_content(&m.parse().unwrap(), "a.bin".into(), plain(), w, h, size)).unwrap();
        assert_eq!(c("image/png", 4, 3, 99), serde_json::json!({ "msgtype": "m.image", "body": "a.bin", "url": "mxc://example.org/abc", "info": { "mimetype": "image/png", "w": 4, "h": 3, "size": 99 } }));
        assert_eq!(c("image/png", 0, 0, 99), serde_json::json!({ "msgtype": "m.image", "body": "a.bin", "url": "mxc://example.org/abc", "info": { "mimetype": "image/png" } }));
        assert_eq!(c("video/mp4", 0, 0, 99), serde_json::json!({ "msgtype": "m.video", "body": "a.bin", "url": "mxc://example.org/abc", "info": { "mimetype": "video/mp4" } }));
        assert_eq!(c("audio/ogg", 0, 0, 99), serde_json::json!({ "msgtype": "m.audio", "body": "a.bin", "url": "mxc://example.org/abc", "info": { "mimetype": "audio/ogg" } }));
        assert_eq!(c("application/pdf", 0, 0, 99), serde_json::json!({ "msgtype": "m.file", "body": "a.bin", "url": "mxc://example.org/abc", "info": { "mimetype": "application/pdf" } }));
    }

    #[test]
    fn encrypted_media_goes_out_as_file_never_url() {
        let file: matrix_sdk::ruma::events::room::EncryptedFile = serde_json::from_value(serde_json::json!({
            "url": "mxc://example.org/enc", "v": "v2", "hashes": { "sha256": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA" }, "iv": "AAAAAAAAAAAAAAAAAAAAAA",
            "key": { "kty": "oct", "key_ops": ["encrypt", "decrypt"], "alg": "A256CTR", "k": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", "ext": true }
        }))
        .unwrap();
        let v = serde_json::to_value(attachment_content(&"image/png".parse().unwrap(), "a.png".into(), MediaSource::Encrypted(Box::new(file)), 0, 0, 0)).unwrap();
        assert!(v.get("url").is_none());
        assert_eq!(v["file"]["url"], "mxc://example.org/enc");
    }

    /// The closures above are only as good as the send path using them.
    #[test]
    fn the_send_path_uses_the_checked_upload() {
        let src = include_str!("room.rs");
        let start = src.find("EchoKind::Attachment(path) => {").expect("the attachment arm");
        let end = start + src[start..].find("fn attach_upload_encrypt").expect("the end of the arm");
        let arm = &src[start..end];
        assert!(arm.contains("attach_upload_checked("), "the arm must upload through the checked upload");
        assert!(arm.contains("|| room.encryption_state(),"), "the re-read before the upload");
        assert!(arm.contains(".send_raw(\"m.room.message\", content)"), "the event goes out with send_raw");
        assert!(!arm.contains(".send_attachment("), "Room::send_attachment decides the encryption once, before the upload");
    }
}

/// "Location is removed from photos you send" (docs/PHOTO-METADATA.md): the upload rule on real photo bytes (the shared
/// vectors testvectors/exif-strip), and every image upload path going through it. Mirrors the web's
/// src/core/exifstrip.test.ts.
#[cfg(test)]
mod photo_location_tests {
    use super::*;

    const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../testvectors/exif-strip");

    fn vectors() -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(format!("{DIR}/vectors.json")).unwrap()).unwrap()
    }
    fn input(name: &str) -> Vec<u8> {
        std::fs::read(format!("{DIR}/in/{name}")).unwrap()
    }
    fn leaks(b: &[u8]) -> Vec<String> {
        let v = vectors();
        let mut needles: Vec<Vec<u8>> = v["secrets"].as_array().unwrap().iter().map(|s| s.as_str().unwrap().chars().map(|c| c as u8).collect()).collect();
        for g in v["gpsRationals"].as_array().unwrap() {
            let h = g.as_str().unwrap();
            needles.push((0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap()).collect());
        }
        needles.into_iter().filter(|n| b.windows(n.len()).any(|w| w == n.as_slice())).map(|n| String::from_utf8_lossy(&n).into_owned()).collect()
    }
    const PHOTOS: [(&str, &str, u16); 12] = [
        ("heic-moov-meta-xml-orient3.heic", "image/heic", 3),
        ("heic-samsung-sefd-uuid-orient3.heic", "image/heic", 3),
        ("jpeg-jfif-thumbs-adobe-orient6.jpg", "image/jpeg", 6),
        ("png-iccp-splt-iend-orient8.png", "image/png", 8),
        ("webp-anim-frame-chunks.webp", "image/webp", 1),
        ("avif-xml-and-uri-items.avif", "image/avif", 1),
        ("jpeg-phone-orient6.jpg", "image/jpeg", 6),
        ("png-exif-text-orient6.png", "image/png", 6),
        ("webp-exif-xmp-orient6.webp", "image/webp", 6),
        ("avif-exif-xmp-orient6.avif", "image/avif", 6),
        ("heic-synthetic-orient3.heic", "image/heic", 3),
        ("jpeg-mpf-second-picture.jpg", "image/jpeg", 8),
    ];

    #[test]
    fn an_uploaded_photo_carries_no_gps_and_keeps_its_orientation() {
        for (name, mime, orient) in PHOTOS {
            let raw = input(name);
            assert!(!leaks(&raw).is_empty(), "{name}: the input is identifying");
            let up = photo_bytes_for_upload(mime, raw).unwrap();
            assert!(leaks(&up).is_empty(), "{name}: the uploaded bytes still carry {:?}", leaks(&up));
            assert_eq!(tinhat_widgets::exifstrip::strip_image_metadata(&up).unwrap().orientation, orient, "{name}: orientation kept");
        }
    }

    /// A phone's HEIC/HEIF/AVIF photo goes as a picture (so it is cleaned), like on the web (format.ts EXT_MIME).
    #[test]
    fn heic_photos_are_pictures() {
        for (ext, m) in [("heic", "image/heic"), ("heif", "image/heif"), ("hif", "image/heif"), ("avif", "image/avif"), ("jpg", "image/jpeg")] {
            assert_eq!(mime_for_ext(ext), m, "{ext}");
        }
        let up = photo_bytes_for_upload(mime_for_ext("heic"), input("heic-synthetic-orient3.heic")).unwrap();
        assert!(leaks(&up).is_empty());
    }

    #[test]
    fn the_rule_sniffs_the_bytes() {
        // a JPEG behind a .png name is still cleaned
        let up = photo_bytes_for_upload("image/png", input("jpeg-phone-orient6.jpg")).unwrap();
        assert!(leaks(&up).is_empty());
    }

    #[test]
    fn not_a_picture_goes_up_as_it_is() {
        let raw = input("text-unchanged.txt");
        for mime in ["application/octet-stream", "application/pdf", "video/mp4", "image/jpeg", ""] {
            assert_eq!(photo_bytes_for_upload(mime, raw.clone()).unwrap(), raw, "{mime}");
        }
        for (name, mime) in [("gif-unchanged.gif", "image/gif"), ("jpeg-clean.jpg", "image/jpeg"), ("png-clean.png", "image/png"), ("webp-simple.webp", "image/webp")] {
            assert_eq!(photo_bytes_for_upload(mime, input(name)).unwrap(), input(name), "{name}");
        }
    }

    /// 12.1.16 2-2: the bytes decide, never the declared type or the extension (testvectors/exif-strip/upload-rule.json,
    /// the web replays the same table). Each case runs with the mime the attachment would get from its name
    /// (mime_for_ext) and with its declared type.
    #[test]
    fn upload_rule_2_2_the_bytes_decide() {
        let doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(format!("{DIR}/upload-rule.json")).unwrap()).unwrap();
        let cases = doc["cases"].as_array().unwrap();
        assert!(cases.len() >= 25);
        for c in cases {
            let file = c["in"].as_str().unwrap();
            let name = c["name"].as_str().unwrap();
            let typ = c["type"].as_str().unwrap();
            let expect = c["expect"].as_str().unwrap();
            let ext = std::path::Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
            for mime in [mime_for_ext(&ext), if typ.is_empty() { "application/octet-stream" } else { typ }] {
                let what = format!("{file} as {name} ({mime})");
                let got = photo_bytes_for_upload(mime, input(file));
                match expect {
                    "refused" => assert_eq!(friendly_send_error(&got.expect_err(&what)), PHOTO_LOCATION_FAIL, "{what}"),
                    "cleaned" => {
                        let got = got.unwrap_or_else(|e| panic!("{what}: {e}"));
                        assert!(got == std::fs::read(format!("{DIR}/out/{file}")).unwrap(), "{what}: not the cleaned bytes");
                        assert!(leaks(&got).is_empty(), "{what}: still carries {:?}", leaks(&got));
                    }
                    _ => assert!(got.unwrap_or_else(|e| panic!("{what}: {e}")) == input(file), "{what}: changed"),
                }
            }
        }
    }

    /// 12.1.16 R3-1: an upload that is NOT a picture (soundboard clip, voice note) refuses picture bytes in any format;
    /// real audio goes up as it is (testvectors/exif-strip/non-picture-upload.json, the web replays it too).
    #[test]
    fn non_picture_uploads_refuse_picture_bytes_r3_1() {
        let doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(format!("{DIR}/non-picture-upload.json")).unwrap()).unwrap();
        let cases = doc["cases"].as_array().unwrap();
        assert!(cases.len() >= 8);
        for c in cases {
            let file = c["in"].as_str().unwrap();
            let got = refuse_picture_bytes(&input(file));
            match c["expect"].as_str().unwrap() {
                "refused" => {
                    let e = got.expect_err(file);
                    assert_eq!(friendly_send_error(&format!("voice: {e}")), PHOTO_LOCATION_FAIL, "{file}: the voice note's words");
                }
                _ => assert!(got.is_ok(), "{file}: real audio must go up"),
            }
        }
        assert_eq!(crate::soundboard::upload_fail_words("picture", 0), PHOTO_LOCATION_FAIL, "the soundboard's words");
    }

    /// R3-1: every fn that sends a request body (reqwest `.body(` / `.multipart(`, a D-Bus body) is a known kind; a new
    /// one has to be looked at: a body that could be a user's file must go through photo_bytes_for_upload (a picture) or
    /// refuse_picture_bytes (not one), then be added here with what it sends.
    #[test]
    fn every_request_body_is_a_known_kind_r3_1() {
        const KNOWN: [(&str, &str); 36] = [
            // mini games: the play API's JSON bodies (moves, tables, wallet) + the OpenID exchanges for a play session
            // (the play client's and the prestige store's): never a user's file
            ("play_net.rs:call", "JSON"), ("play_net.rs:sign_in", "JSON: the OpenID token for a play session"),
            ("prestige.rs:own_session", "JSON: the OpenID token for a play session"),
            ("bridge.rs:body_json", "reads a reply"), ("bridge.rs:do_complete_password_reset", "JSON"),
            ("bridge.rs:do_request_password_reset", "JSON"), ("bridge.rs:gate_login", "JSON"), ("bridge.rs:raw_join", "JSON"),
            ("call_ctl.rs:uplink_probe", "speed test: zeros"), ("flatpak.rs:apply", "D-Bus"), ("flatpak.rs:monitor_loop", "D-Bus"),
            ("games.rs:catalog_post", "JSON"), ("gameservers_ctl.rs:call", "JSON"), ("guests_ctl.rs:call", "JSON"),
            ("invites.rs:call", "JSON"), ("livekit.rs:post_sfu_get", "JSON"), ("logship.rs:ship_bug_report", "app log text"),
            ("logship.rs:ship_previous_session_log", "app log text"), ("moderation.rs:post_json_auth", "JSON"),
            ("notif_actions.rs:session_locked", "D-Bus"), ("notify.rs:body_for_server", "notification text"),
            ("notify.rs:caps", "D-Bus"), ("notify.rs:here", "D-Bus"), ("notify.rs:notify_action_replacing", "notification text"),
            ("notify.rs:notify_call", "notification text"), ("notify.rs:notify_message", "notification text"),
            ("password.rs:change_password", "JSON"), ("presence.rs:api", "JSON"), ("presence.rs:probe_idle", "D-Bus"),
            ("ringglue.rs:decline", "JSON"), ("safety.rs:delete_account_inner", "JSON"), ("safety.rs:send_report", "JSON"),
            ("safety.rs:set_ignored", "JSON"), ("settings.rs:raw_login", "JSON"), ("settings.rs:set_profile_field", "JSON"),
            ("soundboard.rs:upload_clip", "the clip: refuse_picture_bytes"),
        ];
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().and_then(|x| x.to_str()) == Some("rs") {
                    out.push(p);
                }
            }
        }
        // test modules out (the same layout rule as every_image_upload_is_covered)
        fn app_code(s: &str) -> String {
            let lines: Vec<&str> = s.split('\n').collect();
            let (mut out, mut i) = (Vec::new(), 0);
            while i < lines.len() {
                let l = lines[i];
                if l.trim() == "#[cfg(test)]" {
                    let ind = &l[..l.len() - l.trim_start().len()];
                    let mut j = i + 1;
                    if j < lines.len() && lines[j].trim_end().ends_with(';') {
                        i = j + 1;
                        continue;
                    }
                    let close = format!("{ind}}}");
                    while j < lines.len() && lines[j] != close {
                        j += 1;
                    }
                    i = j + 1;
                    continue;
                }
                out.push(l);
                i += 1;
            }
            out.join("\n")
        }
        let mut files = Vec::new();
        walk(std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src")), &mut files);
        let mut sites: Vec<String> = Vec::new();
        for p in files {
            let name = p.file_name().unwrap().to_str().unwrap().to_string();
            let code = app_code(&std::fs::read_to_string(&p).unwrap());
            for pat in [".body(", ".multipart("] {
                for (at, _) in code.match_indices(pat) {
                    let head = &code[..at];
                    let start = head
                        .match_indices(" fn ")
                        .map(|(i, _)| i)
                        .filter(|&i| {
                            let line_start = head[..i].rfind('\n').map_or(0, |n| n + 1);
                            let pre = head[line_start..i].trim();
                            pre.is_empty() || pre.ends_with("pub") || pre.ends_with("async") || pre.ends_with("pub(crate)") || pre.ends_with(')')
                        })
                        .last()
                        .unwrap_or(0);
                    let fname: String = head[start + 4..].chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                    sites.push(format!("{name}:{fname}"));
                }
            }
        }
        sites.sort();
        sites.dedup();
        let mut known: Vec<String> = KNOWN.iter().map(|(s, _)| s.to_string()).collect();
        known.sort();
        assert_eq!(sites, known, "a request body appeared or went: classify it (see the doc comment)");
    }

    /// R3-1: the soundboard clip and the voice note refuse picture bytes before anything is sent.
    #[test]
    fn the_soundboard_and_voice_note_refuse_pictures_first() {
        let sb = include_str!("soundboard.rs");
        let f = &sb[sb.find("async fn upload_clip(").unwrap()..];
        let f = &f[..f.find("\n}\n").unwrap()];
        let r = f.find("crate::room::refuse_picture_bytes(&bytes)").expect("upload_clip refuses pictures");
        assert!(r < f.find("bytes.len() > 3 * 1024 * 1024").unwrap() && r < f.find(".post(&url)").unwrap());
        let room = include_str!("room.rs");
        let v = &room[room.find("EchoKind::Voice(v) => {").unwrap()..];
        let r = v.find("refuse_picture_bytes(&data)").expect("the voice note refuses pictures");
        assert!(r < v.find("upload_encrypted_file(").unwrap() && r < v.find(".upload(&mime, data, None)").unwrap());
    }

    #[test]
    fn a_photo_that_cant_be_cleaned_is_refused_with_the_shared_words() {
        let err = photo_bytes_for_upload("image/jpeg", input("jpeg-bad-length.jpg")).unwrap_err();
        assert_eq!(friendly_send_error(&err), "Couldn't remove this photo's location, so it wasn't sent");
        assert_eq!(PHOTO_LOCATION_FAIL, "Couldn't remove this photo's location, so it wasn't sent");
    }

    /// The chat attachment (attach, paste, drag-drop: all are send_attachment → EchoKind::Attachment) cleans the bytes
    /// before the upload and states the cleaned size.
    #[test]
    fn the_attachment_send_cleans_before_the_upload() {
        let src = include_str!("room.rs");
        let start = src.find("EchoKind::Attachment(path) => {").expect("the attachment arm");
        let end = start + src[start..].find("EchoKind::Voice(v) => {").expect("the end of the arm");
        let arm = &src[start..end];
        let clean = arm.find("let data = photo_bytes_for_upload(mime.as_ref(), data)?;").expect("the arm cleans the bytes");
        let read = arm.find("tokio::fs::read(&pb)").unwrap();
        let upload = arm.find("attach_upload_checked(").unwrap();
        assert!(read < clean && clean < upload, "read → clean → upload");
        assert!(arm.contains("attachment_content(&mime, filename, source, e.w, e.h, size)"), "the cleaned size");
        // the event never states the original file's size (L7's upload-limit check before the read may use it)
        assert!(!arm.contains("e.w, e.h, e.size)"), "never the original file's size in the event");
    }

    #[test]
    fn the_room_and_profile_photos_are_cleaned_too() {
        let room = include_str!("room.rs");
        let f = &room[room.find("pub(crate) async fn do_apply_room_photo(").unwrap()..];
        let f = &f[..f.find("\n}\n").unwrap()];
        assert!(f.find("photo_bytes_for_upload(").unwrap() < f.find(".upload(&mime, data, None)").unwrap());
        let settings = include_str!("settings.rs");
        let g = &settings[settings.find("async fn upload_and_set_avatar(").unwrap()..];
        let g = &g[..g.find("\n}\n").unwrap()];
        assert!(g.find("photo_bytes_for_upload(").unwrap() < g.find(".upload_avatar(&mime, data)").unwrap());
    }

    /// Every media upload cleans pictures first: in the app's code (test items removed), each upload call's own fn calls
    /// photo_bytes_for_upload before it. The only exception: call_ctl.rs's built-in 1x1 PNG (no user picture). Voice
    /// notes share perform_echo, which cleans (audio goes up unchanged). The named sites must all be found, so a moved or
    /// renamed upload is noticed: the chat attachment, the room photo, the profile photo, Memories "Add photos" and the
    /// chat background's own picture (ambience; it is posted in the chat, PRODUCT-CALLS #40d).
    #[test]
    fn every_image_upload_is_covered() {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().and_then(|x| x.to_str()) == Some("rs") {
                    out.push(p);
                }
            }
        }
        // #[cfg(test)] items removed by layout: from the attribute to the first later line that is its indent + "}"
        fn app_code(s: &str) -> String {
            let lines: Vec<&str> = s.split('\n').collect();
            let mut out = Vec::new();
            let mut i = 0;
            while i < lines.len() {
                let l = lines[i];
                if l.trim() == "#[cfg(test)]" {
                    let ind = &l[..l.len() - l.trim_start().len()];
                    let mut j = i + 1;
                    if j < lines.len() && lines[j].trim_end().ends_with(';') {
                        i = j + 1;
                        continue;
                    }
                    let close = format!("{ind}}}");
                    while j < lines.len() && lines[j] != close {
                        j += 1;
                    }
                    i = j + 1;
                    continue;
                }
                out.push(l);
                i += 1;
            }
            out.join("\n")
        }
        let mut files = Vec::new();
        walk(std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src")), &mut files);
        let mut sites: Vec<String> = Vec::new();
        for p in files {
            let name = p.file_name().unwrap().to_str().unwrap().to_string();
            let code = app_code(&std::fs::read_to_string(&p).unwrap());
            // R3-1: a multipart body is an upload too (the soundboard clip): it must refuse pictures
            for pat in [".upload(", ".upload_avatar(", "upload_encrypted_file(", "multipart/form-data"] {
                for (at, _) in code.match_indices(pat) {
                    // the call's own fn: the last `fn name(` line before it
                    let head = &code[..at];
                    let start = head
                        .match_indices(" fn ")
                        .map(|(i, _)| i)
                        .filter(|&i| {
                            let line_start = head[..i].rfind('\n').map_or(0, |n| n + 1);
                            let pre = head[line_start..i].trim();
                            pre.is_empty() || pre.ends_with("pub") || pre.ends_with("async") || pre.ends_with("pub(crate)") || pre.ends_with(')')
                        })
                        .last()
                        .unwrap_or(0);
                    let fname: String = head[start + 4..].chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                    sites.push(format!("{name}:{fname}"));
                    if name == "call_ctl.rs" {
                        assert!(code.contains("upload_avatar(&mime, PNG.to_vec())"), "call_ctl.rs: only the built-in PNG");
                        continue;
                    }
                    assert!(
                        head[start..].contains("photo_bytes_for_upload(") || head[start..].contains("refuse_picture_bytes("),
                        "{name} {fname}: an upload without photo_bytes_for_upload (a picture) or refuse_picture_bytes (not one) before it"
                    );
                }
            }
        }
        sites.sort();
        sites.dedup();
        for want in ["room.rs:perform_echo", "room.rs:do_apply_room_photo", "settings.rs:upload_and_set_avatar", "memories_photos.rs:send_photo", "room.rs:post_background_image", "soundboard.rs:upload_clip"] {
            assert!(sites.iter().any(|s| s == want), "upload site {want} not found: {sites:?}");
        }
    }
}

#[cfg(test)]
mod thread_redaction_tests {
    use super::*;

    /// threads: a deleted thread reply keeps its thread relation (it stays in its thread, never a stray main row); a
    /// deleted main message keeps nothing.
    #[test]
    fn a_redacted_thread_reply_stays_in_its_thread() {
        let red = serde_json::json!({ "type": "m.room.redaction", "event_id": "$red", "redacts": "$t1", "content": {} });
        let mut reply = serde_json::json!({ "event_id": "$t1", "type": "m.room.message",
            "content": { "body": "x", "msgtype": "m.text", "m.relates_to": { "rel_type": "m.thread", "event_id": "$root", "is_falling_back": true, "m.in_reply_to": { "event_id": "$root" } } } });
        stamp_redacted(&mut reply, &red);
        assert_eq!(reply["content"].get("body"), None);
        assert_eq!(tinhat_widgets::threads::thread_of(&reply["content"]), "$root");
        assert!(reply["unsigned"]["redacted_because"].is_object());
        let plan = crate::threads_rows::thread_plan(&[reply.clone()], None);
        assert!(!plan.in_view[0], "not a main-timeline row");
        let mut main = serde_json::json!({ "event_id": "$m", "type": "m.room.message",
            "content": { "body": "y", "m.relates_to": { "m.in_reply_to": { "event_id": "$x" } } } });
        stamp_redacted(&mut main, &red);
        assert_eq!(main["content"], serde_json::json!({}));
    }
}

/// A room link to a room this account isn't in (PM 2026-11-14): `open` waits this long for the room to reach the local
/// store, then sets this status, which ConversationView (RoomMissing.qml) shows with a way back. Tin Hat Web's
/// roomMissing.ts; shared vectors testvectors/room-missing.
const ROOM_MISSING_WAIT_MS: u64 = 10_000;
const ROOM_MISSING_EXPLANATION: &str = "You're not in it, or it doesn't exist. Ask someone in it to invite you.";
/// A sync pass has completed for this session (bridge.rs sets it after each good pass, clears it at sign-out).
pub(crate) static ROOM_SYNCED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether an open room should stop loading and say it can't be opened (the web's roomMissing).
fn room_missing(known: bool, synced: bool, elapsed_ms: u64) -> bool {
    !known && synced && elapsed_ms >= ROOM_MISSING_WAIT_MS
}

#[cfg(test)]
mod room_missing_tests {
    const VECTORS: &str = include_str!("../../../testvectors/room-missing/vectors.json");

    #[test]
    fn room_missing_words_and_wait_are_the_vectors() {
        let v: serde_json::Value = serde_json::from_str(VECTORS).unwrap();
        assert_eq!(super::ROOM_MISSING_WAIT_MS, v["waitMs"].as_u64().unwrap());
        assert_eq!(super::ROOM_MISSING_EXPLANATION, v["words"]["explanation"].as_str().unwrap());
        for c in v["cases"].as_array().unwrap() {
            let got = super::room_missing(c["known"].as_bool().unwrap(), c["synced"].as_bool().unwrap(), c["elapsedMs"].as_u64().unwrap());
            assert_eq!(got, c["missing"].as_bool().unwrap(), "{}", c["name"]);
        }
        let src = include_str!("room.rs");
        let src = &src[..src.find("mod room_missing_tests").unwrap()];
        assert!(src.contains("while found.is_none() && !room_missing(false, ROOM_SYNCED.load(Ordering::Relaxed), waited) {"), "open waits by the rule");
        assert!(src.contains("set_status(QString::from(ROOM_MISSING_EXPLANATION))"), "the status carries the words");
        assert!(src.contains("o.as_mut().open(&QString::from(&again));"), "a room that arrives later still opens");
        assert!(src.contains("let found = found.or_else(|| client.get_room(&rid));\n            let Some(room) = found else {"), "one more look after the wait (review: the sync's boundary)");
        assert!(!src.contains(&["on this device ", "yet — try again"].concat()), "the old 'yet' words are gone");
        // the shared row (12.1.17): "synced" = the first sync from the NETWORK; ROOM_SYNCED is set only by a good network
        // sync pass of the sync loop (never by the store a start restores), so a cache start keeps loading
        assert!(v["synced"].as_str().unwrap().contains("NETWORK"));
        assert!(v["cases"].as_array().unwrap().iter().any(|c| c["cacheSynced"] == true && c["synced"] == false && c["missing"] == false));
        let bridge = include_str!("bridge.rs");
        assert_eq!(bridge.matches("ROOM_SYNCED.store(true").count(), 1, "one place sets it: after a network sync pass");
        assert!(bridge.contains("crate::room::ROOM_SYNCED.store(true, std::sync::atomic::Ordering::Relaxed);"), "a sync pass sets it");
        assert!(bridge.contains("crate::room::ROOM_SYNCED.store(false, std::sync::atomic::Ordering::Relaxed);"), "sign-out clears it");
    }

    /// The byte copy equals the web's vectors (`TINHAT_WEB_DIR` = a tinhat-web tree, else `$HOME/tinhat-web`;
    /// skipped when neither has the file).
    #[test]
    fn room_missing_vectors_are_the_webs() {
        let dir = std::env::var_os("TINHAT_WEB_DIR").map(std::path::PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("tinhat-web")));
        let Some(dir) = dir else { return };
        let path = dir.join("testvectors/room-missing/vectors.json");
        match std::fs::read_to_string(&path) {
            Ok(theirs) => assert!(VECTORS == theirs, "testvectors/room-missing differs from {}", path.display()),
            Err(_) => eprintln!("SKIPPED: no web vectors at {}", path.display()),
        }
    }
}

// ---- chat backgrounds (docs/AMBIENCE.md) ----------------------------------------------------------------------------

extern "C" {
    fn tinhat_ambience_reencode(src: *const std::os::raw::c_char, dst: *const std::os::raw::c_char, max_side: i32, max_bytes: i64, qualities: *const f64, nq: i32, shrink: f64, out_w: *mut i32, out_h: *mut i32) -> i64;
}

/// Re-encode a picked image for a chat background (ambience_image.cpp): `(bytes, w, h)`.
fn ambience_reencode(src: &str, dst: &std::path::Path) -> Result<(u64, u32, u32), String> {
    use tinhat_widgets::ambience as amb;
    let s = std::ffi::CString::new(src).map_err(|e| e.to_string())?;
    let d = std::ffi::CString::new(dst.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
    let q = amb::IMAGE_QUALITIES;
    let (mut w, mut h) = (0i32, 0i32);
    // SAFETY: valid NUL-terminated strings and a live array for the call's duration; out pointers to locals.
    let n = unsafe { tinhat_ambience_reencode(s.as_ptr(), d.as_ptr(), amb::IMAGE_MAX_SIDE as i32, amb::IMAGE_MAX_BYTES as i64, q.as_ptr(), q.len() as i32, amb::IMAGE_SHRINK, &mut w, &mut h) };
    if n < 0 {
        return Err("not an image, or too large".into());
    }
    Ok((n as u64, w.max(0) as u32, h.max(0) as u32))
}

/// The background's own image as a local file URL + its colour cells (every pixel; ambience_image.cpp), when the shared
/// rule (ambience::image_shown) says it may show here: the setter's own image message in this room, not redacted,
/// decrypted on this device. ("", "") = show the preset.
async fn background_image_file(room: &Room, bg_json: &str) -> (String, String) {
    use tinhat_widgets::ambience as amb;
    let none = (String::new(), String::new());
    let Ok(v) = serde_json::from_str::<serde_json::Value>(bg_json) else { return none };
    let bg = amb::parse_background(v.get("content").unwrap_or(&serde_json::Value::Null), v.get("sender").and_then(|s| s.as_str()).unwrap_or(""));
    let Some(eid_s) = bg.image.clone().filter(|_| bg.kind == amb::BackgroundKind::Image) else { return none };
    let Ok(eid) = matrix_sdk::ruma::EventId::parse(&eid_s) else { return none };
    let Ok(ev) = room.event(&eid, None).await else { return none };
    let Ok(raw) = ev.raw().deserialize_as::<serde_json::Value>() else { return none };
    let ty = raw.get("type").and_then(|t| t.as_str()).unwrap_or("").to_string();
    let content = raw.get("content").cloned().unwrap_or_default();
    let info = amb::ImageEvent {
        r#type: ty.clone(),
        sender: raw.get("sender").and_then(|s| s.as_str()).unwrap_or("").to_string(),
        room: room.room_id().to_string(),
        msgtype: content.get("msgtype").and_then(|m| m.as_str()).unwrap_or("").to_string(),
        redacted: raw.get("unsigned").and_then(|u| u.get("redacted_because")).is_some(),
        decrypted: ty == "m.room.message",
    };
    if !amb::image_shown(&bg, room.room_id().as_str(), Some(&info)) {
        return none;
    }
    let mime = content.get("info").and_then(|i| i.get("mimetype")).and_then(|m| m.as_str()).map(str::to_string);
    // fetch_media hands back a file URL (what QML loads), not a path
    let Ok(url) = fetch_media(content, &eid_s, mime.as_deref(), MediaUse::InlineImage).await else { return none };
    let u2 = url.clone();
    let cells = tokio::task::spawn_blocking(move || media_cells(&u2)).await.ok().flatten();
    // not decodable here, or bigger than 4096 px: the preset (never an image whose colours weren't measured)
    let Some(cells) = cells else { return none };
    (url, cells)
}

/// The colour cells of a picture fetch_media cached, from the file URL it returns. The cell reader opens a file PATH:
/// handed the URL it measured nothing, and the desktop showed only the fallback scene behind every own picture
/// (round 6, G-ambience A6).
fn media_cells(url: &str) -> Option<String> {
    ambience_cells(&media_path(url).to_string_lossy())
}

/// The exact inverse of platform::file_url on each OS. Linux: the raw `file://` + path, never percent-encoded (a URL
/// parser would cut a cache directory at a `#` or decode a `%20` in it); Windows: url::Url::from_file_path's URL.
fn media_path(url: &str) -> std::path::PathBuf {
    #[cfg(windows)]
    {
        crate::memories_photos::local_path(url)
    }
    #[cfg(not(windows))]
    {
        std::path::PathBuf::from(crate::platform::path_from_file_url(url))
    }
}

extern "C" {
    fn tinhat_ambience_cells(path: *const std::os::raw::c_char, max_side: i32, out: *mut f64, cap: i32) -> i32;
}

/// The image's 8 × 8 colour cells over EVERY pixel of its first frame, as the JSON ambience::scrim_for_cells takes.
fn ambience_cells(path: &str) -> Option<String> {
    let p = std::ffi::CString::new(path).ok()?;
    let mut out = vec![0f64; 64 * 6];
    // SAFETY: a valid NUL-terminated path and a buffer of `cap` cells (6 doubles each) for the call's duration.
    let n = unsafe { tinhat_ambience_cells(p.as_ptr(), 4096, out.as_mut_ptr(), 64) };
    if n <= 0 {
        return None;
    }
    let cells: Vec<serde_json::Value> = (0..n as usize).map(|i| { let c = &out[i * 6..i * 6 + 6]; serde_json::json!([[c[0], c[1], c[2]], [c[3], c[4], c[5]]]) }).collect();
    Some(serde_json::Value::Array(cells).to_string())
}

#[cfg(test)]
mod ambience_cells_tests {
    use super::ambience_cells;

    const PNG_1X1: [u8; 69] = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, 0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xff, 0xff, 0x3f, 0x00, 0x05, 0xfe, 0x02, 0xfe, 0x0d, 0xef, 0x46, 0xb8, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82];

    fn write(name: &str, bytes: &[u8]) -> String {
        let p = std::env::temp_dir().join(format!("tinhat-test-{}-{name}", std::process::id()));
        std::fs::write(&p, bytes).unwrap();
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn background_cells_come_from_the_url_fetch_media_returns() {
        // round 6 (G-ambience A6): fetch_media returns a file URL; the own picture's cells must be measured from it, or
        // the desktop draws only the fallback scene behind every own picture (2e1858b61 did exactly that)
        let ok = write("bg own picture.png", &PNG_1X1);
        let url = crate::platform::file_url(std::path::Path::new(&ok));
        assert!(url.starts_with("file:"), "{url}");
        assert!(super::media_cells(&url).is_some(), "the cells of the picture behind {url}");
        // the cell reader itself takes a path (why the URL has to be turned into one)
        assert!(ambience_cells(&url).is_none());
        // a media cache directory whose own name holds a "#" or a "%20" (the URL is never percent-encoded on Linux)
        let dir = std::env::temp_dir().join(format!("tinhat-test-{} cache#1 %20x", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let odd = dir.join("bg.png");
        std::fs::write(&odd, PNG_1X1).unwrap();
        let odd_url = crate::platform::file_url(&odd);
        assert!(super::media_cells(&odd_url).is_some(), "the cells of the picture behind {odd_url}");
        let _ = std::fs::remove_file(ok);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn ambience_image_size_is_checked_before_decoding() {
        // 3-6 (desktop twin): a real 1 × 1 picture is measured …
        let ok = write("ok.png", &PNG_1X1);
        assert!(ambience_cells(&ok).is_some());
        // … a 30000 × 30000 header is refused before any pixel is decoded …
        let mut bomb = PNG_1X1.to_vec();
        bomb[16..20].copy_from_slice(&30000u32.to_be_bytes());
        bomb[20..24].copy_from_slice(&30000u32.to_be_bytes());
        let bomb_p = write("bomb.png", &bomb);
        assert!(ambience_cells(&bomb_p).is_none());
        // … and so is anything whose header doesn't say its size
        let junk = write("junk.bin", b"not an image at all, just some bytes");
        assert!(ambience_cells(&junk).is_none());
        for p in [ok, bomb_p, junk] {
            let _ = std::fs::remove_file(p);
        }
    }
}

// 12.1.17 round 2 (assembly-1217 review #2/#3): where a read receipt goes when the view's newest event is a photo the chat
// hides (it is in Memories). Before round 2 a thread's panel showing only its root returned without a receipt, and a
// thread's hidden tail was never a target: the thread stayed unread on every other device.
#[cfg(test)]
mod receipt_target_tests {
    use super::{nonrow_key, receipt_target};

    fn t(id: &str, ts: u64) -> Option<(String, u64)> {
        Some((id.to_string(), ts))
    }

    #[test]
    fn main_timeline_the_newer_of_the_newest_row_and_the_hidden_tail() {
        assert_eq!(receipt_target(t("$row", 10), t("$hidden", 20), ""), Some("$hidden".into()));
        assert_eq!(receipt_target(t("$row", 30), t("$hidden", 20), ""), Some("$row".into()));
        assert_eq!(receipt_target(t("$row", 10), None, ""), Some("$row".into()));
        // review r2 #1: every row hidden: a main receipt on the hidden tail (never the empty-room fallback, which may
        // land on a thread's event)
        assert_eq!(receipt_target(None, t("$hidden", 20), ""), Some("$hidden".into()));
        // no bubble and nothing hidden: the caller's empty-room path
        assert_eq!(receipt_target(None, None, ""), None);
    }

    #[test]
    fn a_threads_only_reply_hidden_is_still_read() {
        // the panel shows only its root; its one reply is a photo the chat hides → the receipt goes to that reply
        assert_eq!(receipt_target(t("$root", 5), t("$reply", 9), "$root"), Some("$reply".into()));
        assert_eq!(receipt_target(None, t("$reply", 9), "$root"), Some("$reply".into()));
        // a visible reply older than the hidden tail: the tail
        assert_eq!(receipt_target(t("$r1", 7), t("$reply", 9), "$root"), Some("$reply".into()));
        // nothing but the root: nothing to read (a thread receipt on the root would be refused)
        assert_eq!(receipt_target(t("$root", 5), None, "$root"), None);
        // an empty id never counts
        assert_eq!(receipt_target(t("$root", 5), t("", 9), "$root"), None);
    }

    #[test]
    fn each_view_keeps_its_own_hidden_tail() {
        assert_eq!(nonrow_key("!r:x", ""), "!r:x");
        assert_eq!(nonrow_key("!r:x", "$root"), "!r:x\u{0}$root");
        assert_ne!(nonrow_key("!r:x", "$a"), nonrow_key("!r:x", "$b"));
    }
}

// 12.1.17 round 2 (assembly-1217 review #8): a receipt on a photo the chat hides shows on the newest server row at or
// before the photo's time (before round 2 it was dropped: that reader's "seen by" face disappeared).
#[cfg(test)]
mod row_before_tests {
    use super::row_before;

    #[test]
    fn a_hidden_photos_receipt_lands_on_the_row_before_it_by_position() {
        // 4 rows (oldest first); a photo with 1 row newer than it sits between rows 2 and 3 → row 2
        assert_eq!(row_before(4, 1), Some(2));
        assert_eq!(row_before(4, 0), Some(3)); // the photo is the newest: the last row
        assert_eq!(row_before(4, 4), None); // older than every loaded row
        assert_eq!(row_before(0, 0), None);
        assert_eq!(row_before(3, 9), None);
    }
}
