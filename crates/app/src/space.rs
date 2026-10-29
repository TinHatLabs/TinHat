//! `SpaceController` — QML id `spaceCtl` (RBAC, docs/RBAC.md §5.3).
//!
//! The data layer of the Space settings window: one open space at a time (`open(spaceId)`),
//! four JSON models (summary / members / roles / rooms) plus the invite picker's directory
//! search, and every action the window performs — settings, invites, kick/ban/unban (with the
//! cascade into the space's joined child rooms), role editing and assignment, "Set up roles"
//! and the power-level reconciler (`syncRooms`).
//!
//! Shape of every action: check the permission locally (a human message into `lastError`),
//! do the SDK call, wait briefly for the local store to catch up, `refresh()`, set `status`.
//! `rev` bumps whenever a model property changes; `statusRev` / `errorRev` bump on every set
//! so QML can toast repeats of the same text.
//!
//! Permission checks work in BOTH modes (`roles::effective_perms_with`): roles when the space
//! has them, the space's power levels otherwise — so Overview / Members are usable on a space
//! that never ran "Set up roles", and the Roles tab offers exactly that.

use core::pin::Pin;
use std::collections::BTreeSet;

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use matrix_sdk::ruma::events::room::member::MembershipState;
use matrix_sdk::ruma::events::StateEventType;
use matrix_sdk::ruma::{RoomId, UserId};
use matrix_sdk::{Client, Room, RoomMemberships, RoomState};
use serde_json::{json, Value};

use crate::roles::{self, Perms};
use crate::roles_model::{self, Role, RolesState, EVERYONE, MEMBER_ROLES_TYPE, ROLE_TYPE};

#[cxx_qt::bridge]
pub mod qobject {
    extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[auto_cxx_name] // snake_case Rust -> camelCase QML
    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(QString, space_id)] //      QML: spaceId — the open space, "" when closed
        #[qproperty(bool, busy)] //             QML: busy — any load/action in flight
        #[qproperty(i32, rev)] //               QML: rev — bumps on any data change
        #[qproperty(QString, status)] //        QML: status — last success text
        #[qproperty(i32, status_rev)] //        QML: statusRev — bumps on every status set
        #[qproperty(QString, last_error)] //    QML: lastError — last failure text (user-facing)
        #[qproperty(i32, error_rev)] //         QML: errorRev — bumps on every lastError set
        #[qproperty(bool, roles_active)] //     QML: rolesActive
        #[qproperty(i32, my_rank)] //           QML: myRank
        #[qproperty(QString, summary_json)] //  QML: summaryJson — {id,name,topic,avatar,alias,link,joinRule,…}
        #[qproperty(QString, members_json)] //  QML: membersJson — [{mxid,name,avatar,membership,rank,roles,topRole,…}]
        #[qproperty(QString, roles_json)] //    QML: rolesJson — [{id,name,color,rank,permissions,memberCount,editable,deletable,isEveryone}]
        #[qproperty(QString, my_perms_json)] // QML: myPermsJson — {"<key>":bool,…}
        #[qproperty(QString, permission_catalog_json)] // QML: permissionCatalogJson — static catalogue
        #[qproperty(QString, rooms_json)] //    QML: roomsJson — [{id,name,avatar,joined,synced,canSync}]
        #[qproperty(QString, user_search_json)] // QML: userSearchJson — [{mxid,name,avatar,alreadyMember}]
        type SpaceController = super::SpaceControllerRust;

        /// QML: open(spaceId) — track a space and load everything.
        #[qinvokable]
        fn open(self: Pin<&mut SpaceController>, space_id: &QString);
        /// QML: close() — forget the open space and clear the models.
        #[qinvokable]
        fn close(self: Pin<&mut SpaceController>);
        /// QML: refresh() — reload every model of the open space.
        #[qinvokable]
        fn refresh(self: Pin<&mut SpaceController>);
        /// QML: applyName(name)
        #[qinvokable]
        fn apply_name(self: Pin<&mut SpaceController>, name: &QString);
        /// QML: applyTopic(topic)
        #[qinvokable]
        fn apply_topic(self: Pin<&mut SpaceController>, topic: &QString);
        /// QML: applyPhoto(fileUrl) — "" removes the photo.
        #[qinvokable]
        fn apply_photo(self: Pin<&mut SpaceController>, file_url: &QString);
        /// QML: applyJoinRule(rule) — "public" | "invite".
        #[qinvokable]
        fn apply_join_rule(self: Pin<&mut SpaceController>, rule: &QString);
        /// QML: invite(mxid)
        #[qinvokable]
        fn invite(self: Pin<&mut SpaceController>, mxid: &QString);
        /// QML: searchUsers(query) — user directory; results land in userSearchJson.
        #[qinvokable]
        fn search_users(self: Pin<&mut SpaceController>, query: &QString);
        /// QML: kick(mxid, reason) — from the open space + every joined child room where allowed.
        #[qinvokable]
        fn kick(self: Pin<&mut SpaceController>, mxid: &QString, reason: &QString);
        /// QML: ban(mxid, reason)
        #[qinvokable]
        fn ban(self: Pin<&mut SpaceController>, mxid: &QString, reason: &QString);
        /// QML: unban(mxid)
        #[qinvokable]
        fn unban(self: Pin<&mut SpaceController>, mxid: &QString);
        /// QML: kickIn(spaceId, mxid, reason) — explicit target, no open() needed (call menu).
        #[qinvokable]
        fn kick_in(self: Pin<&mut SpaceController>, space_id: &QString, mxid: &QString, reason: &QString);
        /// QML: banIn(spaceId, mxid, reason)
        #[qinvokable]
        fn ban_in(self: Pin<&mut SpaceController>, space_id: &QString, mxid: &QString, reason: &QString);
        /// QML: setMemberRoles(mxid, rolesJsonArray) — replace someone's assigned roles.
        #[qinvokable]
        fn set_member_roles(self: Pin<&mut SpaceController>, mxid: &QString, roles_json: &QString);
        /// QML: saveRole(roleJson) — {id:"" → create, name, color, rank, permissions:[…]}.
        #[qinvokable]
        fn save_role(self: Pin<&mut SpaceController>, role_json: &QString);
        /// QML: deleteRole(roleId)
        #[qinvokable]
        fn delete_role(self: Pin<&mut SpaceController>, role_id: &QString);
        /// QML: setupRoles() — template + migration + derive + write.
        #[qinvokable]
        fn setup_roles(self: Pin<&mut SpaceController>);
        /// QML: syncRooms() — derive + write power levels to the space and every child where allowed.
        #[qinvokable]
        fn sync_rooms(self: Pin<&mut SpaceController>);
        /// QML: leave() — leave the open space (children are left alone).
        #[qinvokable]
        fn leave(self: Pin<&mut SpaceController>);
    }

    impl cxx_qt::Threading for SpaceController {}
}

pub struct SpaceControllerRust {
    space_id: QString,
    busy: bool,
    rev: i32,
    status: QString,
    status_rev: i32,
    last_error: QString,
    error_rev: i32,
    roles_active: bool,
    my_rank: i32,
    summary_json: QString,
    members_json: QString,
    roles_json: QString,
    my_perms_json: QString,
    permission_catalog_json: QString,
    rooms_json: QString,
    user_search_json: QString,
    /// Loads + actions in flight (`busy` = > 0).
    inflight: i32,
    /// Monotonic search ticket so a slow directory answer can't overwrite a newer one.
    search_seq: u64,
}

impl Default for SpaceControllerRust {
    fn default() -> Self {
        Self {
            space_id: QString::default(),
            busy: false,
            rev: 0,
            status: QString::default(),
            status_rev: 0,
            last_error: QString::default(),
            error_rev: 0,
            roles_active: false,
            my_rank: 0,
            summary_json: QString::from("{}"),
            members_json: QString::from("[]"),
            roles_json: QString::from("[]"),
            my_perms_json: QString::from("{}"),
            permission_catalog_json: QString::from(&roles_model::permission_catalog_json()),
            rooms_json: QString::from("[]"),
            user_search_json: QString::from("[]"),
            inflight: 0,
            search_seq: 0,
        }
    }
}

/// Everything `refresh()` computes, applied to the qproperties in one go.
struct Snapshot {
    summary: String,
    members: String,
    roles: String,
    my_perms: String,
    rooms: String,
    roles_active: bool,
    my_rank: i32,
}

impl qobject::SpaceController {
    fn bump_rev(mut self: Pin<&mut Self>) {
        let r = self.rev().wrapping_add(1);
        self.as_mut().set_rev(r);
    }

    fn set_status_text(mut self: Pin<&mut Self>, msg: &str) {
        self.as_mut().set_status(QString::from(msg));
        let r = self.status_rev().wrapping_add(1);
        self.as_mut().set_status_rev(r);
    }

    fn set_error_text(mut self: Pin<&mut Self>, msg: &str) {
        eprintln!("[space] {msg}");
        self.as_mut().set_last_error(QString::from(msg));
        let r = self.error_rev().wrapping_add(1);
        self.as_mut().set_error_rev(r);
    }

    fn begin(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().inflight += 1;
        self.as_mut().set_busy(true);
    }

    fn end(mut self: Pin<&mut Self>) {
        let n = (self.rust().inflight - 1).max(0);
        self.as_mut().rust_mut().inflight = n;
        self.as_mut().set_busy(n > 0);
    }

    fn clear_models(mut self: Pin<&mut Self>) {
        self.as_mut().set_summary_json(QString::from("{}"));
        self.as_mut().set_members_json(QString::from("[]"));
        self.as_mut().set_roles_json(QString::from("[]"));
        self.as_mut().set_my_perms_json(QString::from("{}"));
        self.as_mut().set_rooms_json(QString::from("[]"));
        self.as_mut().set_user_search_json(QString::from("[]"));
        self.as_mut().set_roles_active(false);
        self.as_mut().set_my_rank(0);
        self.as_mut().bump_rev();
    }

    pub fn open(mut self: Pin<&mut Self>, space_id: &QString) {
        let sid = space_id.to_string();
        if sid != self.space_id().to_string() {
            self.as_mut().set_space_id(QString::from(&sid));
            self.as_mut().set_last_error(QString::default());
            self.as_mut().clear_models();
        }
        if sid.is_empty() {
            return;
        }
        self.refresh();
    }

    pub fn close(mut self: Pin<&mut Self>) {
        self.as_mut().set_space_id(QString::default());
        self.as_mut().clear_models();
    }

    pub fn refresh(mut self: Pin<&mut Self>) {
        let sid = self.space_id().to_string();
        if sid.is_empty() {
            return;
        }
        self.as_mut().begin();
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let snap = build_snapshot(&sid).await;
            let built_for = sid.clone();
            let _ = qt.queue(move |mut o| {
                o.as_mut().end();
                if o.space_id().to_string() != built_for {
                    return; // the user has since opened another space
                }
                match snap {
                    Ok(s) => {
                        o.as_mut().set_summary_json(QString::from(&s.summary));
                        o.as_mut().set_members_json(QString::from(&s.members));
                        o.as_mut().set_roles_json(QString::from(&s.roles));
                        o.as_mut().set_my_perms_json(QString::from(&s.my_perms));
                        o.as_mut().set_rooms_json(QString::from(&s.rooms));
                        o.as_mut().set_roles_active(s.roles_active);
                        o.as_mut().set_my_rank(s.my_rank);
                        o.as_mut().bump_rev();
                    }
                    Err(e) => o.as_mut().set_error_text(&e),
                }
            });
        });
    }

    /// Run one action off-thread: `Ok(text)` → `status`, `Err(text)` → `lastError`; then a
    /// refresh of the open space either way.
    fn run_action(
        mut self: Pin<&mut Self>,
        fut: impl std::future::Future<Output = Result<String, String>> + Send + 'static,
    ) {
        self.as_mut().set_last_error(QString::default());
        self.as_mut().begin();
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let res = fut.await;
            let _ = qt.queue(move |mut o| {
                o.as_mut().end();
                match res {
                    Ok(msg) => {
                        if !msg.is_empty() {
                            o.as_mut().set_status_text(&msg);
                        }
                    }
                    Err(e) => o.as_mut().set_error_text(&e),
                }
                o.as_mut().refresh();
            });
        });
    }

    fn open_space_or_error(mut self: Pin<&mut Self>) -> Option<String> {
        let sid = self.space_id().to_string();
        if sid.is_empty() {
            self.as_mut().set_error_text("No space is open");
            return None;
        }
        Some(sid)
    }

    pub fn apply_name(mut self: Pin<&mut Self>, name: &QString) {
        let Some(sid) = self.as_mut().open_space_or_error() else { return };
        let name = name.to_string();
        self.run_action(async move { do_apply_name(&sid, &name).await });
    }

    pub fn apply_topic(mut self: Pin<&mut Self>, topic: &QString) {
        let Some(sid) = self.as_mut().open_space_or_error() else { return };
        let topic = topic.to_string();
        self.run_action(async move { do_apply_topic(&sid, &topic).await });
    }

    pub fn apply_photo(mut self: Pin<&mut Self>, file_url: &QString) {
        let Some(sid) = self.as_mut().open_space_or_error() else { return };
        let path = file_url.to_string();
        self.run_action(async move { do_apply_photo(&sid, &path).await });
    }

    pub fn apply_join_rule(mut self: Pin<&mut Self>, rule: &QString) {
        let Some(sid) = self.as_mut().open_space_or_error() else { return };
        let rule = rule.to_string();
        self.run_action(async move { do_apply_join_rule(&sid, &rule).await });
    }

    pub fn invite(mut self: Pin<&mut Self>, mxid: &QString) {
        let Some(sid) = self.as_mut().open_space_or_error() else { return };
        let mxid = mxid.to_string();
        self.run_action(async move { do_invite(&sid, &mxid).await });
    }

    pub fn search_users(mut self: Pin<&mut Self>, query: &QString) {
        let q = query.to_string().trim().to_string();
        let seq = self.rust().search_seq.wrapping_add(1);
        self.as_mut().rust_mut().search_seq = seq;
        if q.is_empty() {
            self.as_mut().set_user_search_json(QString::from("[]"));
            self.as_mut().bump_rev();
            return;
        }
        let sid = self.space_id().to_string();
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let json_out = do_search_users(&sid, &q).await;
            let _ = qt.queue(move |mut o| {
                if o.rust().search_seq != seq {
                    return; // a newer query is in flight
                }
                o.as_mut().set_user_search_json(QString::from(&json_out));
                o.as_mut().bump_rev();
            });
        });
    }

    pub fn kick(mut self: Pin<&mut Self>, mxid: &QString, reason: &QString) {
        let Some(sid) = self.as_mut().open_space_or_error() else { return };
        let (mxid, reason) = (mxid.to_string(), reason.to_string());
        self.run_action(async move { do_membership(&sid, &mxid, &reason, MemberAction::Kick).await });
    }

    pub fn ban(mut self: Pin<&mut Self>, mxid: &QString, reason: &QString) {
        let Some(sid) = self.as_mut().open_space_or_error() else { return };
        let (mxid, reason) = (mxid.to_string(), reason.to_string());
        self.run_action(async move { do_membership(&sid, &mxid, &reason, MemberAction::Ban).await });
    }

    pub fn unban(mut self: Pin<&mut Self>, mxid: &QString) {
        let Some(sid) = self.as_mut().open_space_or_error() else { return };
        let mxid = mxid.to_string();
        self.run_action(async move { do_membership(&sid, &mxid, "", MemberAction::Unban).await });
    }

    pub fn kick_in(self: Pin<&mut Self>, space_id: &QString, mxid: &QString, reason: &QString) {
        let (sid, mxid, reason) = (space_id.to_string(), mxid.to_string(), reason.to_string());
        self.run_action(async move { do_membership(&sid, &mxid, &reason, MemberAction::Kick).await });
    }

    pub fn ban_in(self: Pin<&mut Self>, space_id: &QString, mxid: &QString, reason: &QString) {
        let (sid, mxid, reason) = (space_id.to_string(), mxid.to_string(), reason.to_string());
        self.run_action(async move { do_membership(&sid, &mxid, &reason, MemberAction::Ban).await });
    }

    pub fn set_member_roles(mut self: Pin<&mut Self>, mxid: &QString, roles_json: &QString) {
        let Some(sid) = self.as_mut().open_space_or_error() else { return };
        let (mxid, roles_json) = (mxid.to_string(), roles_json.to_string());
        self.run_action(async move { do_set_member_roles(&sid, &mxid, &roles_json).await });
    }

    pub fn save_role(mut self: Pin<&mut Self>, role_json: &QString) {
        let Some(sid) = self.as_mut().open_space_or_error() else { return };
        let role_json = role_json.to_string();
        self.run_action(async move { do_save_role(&sid, &role_json).await });
    }

    pub fn delete_role(mut self: Pin<&mut Self>, role_id: &QString) {
        let Some(sid) = self.as_mut().open_space_or_error() else { return };
        let role_id = role_id.to_string();
        self.run_action(async move { do_delete_role(&sid, &role_id).await });
    }

    pub fn setup_roles(mut self: Pin<&mut Self>) {
        let Some(sid) = self.as_mut().open_space_or_error() else { return };
        self.run_action(async move { do_setup_roles(&sid).await });
    }

    pub fn sync_rooms(mut self: Pin<&mut Self>) {
        let Some(sid) = self.as_mut().open_space_or_error() else { return };
        self.run_action(async move { do_sync_rooms(&sid).await });
    }

    pub fn leave(mut self: Pin<&mut Self>) {
        let Some(sid) = self.as_mut().open_space_or_error() else { return };
        self.as_mut().set_last_error(QString::default());
        self.as_mut().begin();
        let qt = self.qt_thread();
        crate::bridge::runtime().spawn(async move {
            let res = do_leave(&sid).await;
            let left = sid.clone();
            let _ = qt.queue(move |mut o| {
                o.as_mut().end();
                match res {
                    Ok(msg) => {
                        o.as_mut().set_status_text(&msg);
                        if o.space_id().to_string() == left {
                            o.as_mut().close();
                        }
                    }
                    Err(e) => {
                        o.as_mut().set_error_text(&e);
                        o.as_mut().refresh();
                    }
                }
            });
        });
    }
}

// ---- snapshot -------------------------------------------------------------------------------

fn membership_str(state: RoomState) -> &'static str {
    match state {
        RoomState::Joined => "join",
        RoomState::Invited => "invite",
        RoomState::Knocked => "knock",
        RoomState::Banned => "ban",
        RoomState::Left => "leave",
    }
}

/// The space's shareable link (share_link.rs: this server's web app, matrix.to for another server's space).
fn permalink(room_id: &str, alias: &str) -> String {
    crate::share_link::share_link_now(room_id, alias, "")
}

fn join_rule_str(room: &Room) -> &'static str {
    use matrix_sdk::ruma::room::JoinRule;
    match room.join_rule() {
        Some(JoinRule::Public) => "public",
        Some(JoinRule::Knock) => "knock",
        Some(JoinRule::Restricted(_)) | Some(JoinRule::KnockRestricted(_)) => "restricted",
        _ => "invite",
    }
}

async fn room_display_name(room: &Room) -> String {
    match room.display_name().await {
        Ok(d) => d.to_string(),
        Err(_) => room
            .name()
            .or_else(|| room.canonical_alias().map(|a| a.to_string()))
            .unwrap_or_else(|| room.room_id().to_string()),
    }
}

async fn room_avatar(client: &Client, room: &Room) -> String {
    match room.avatar_url() {
        Some(mxc) => crate::bridge::resolve_avatar(client, &mxc).await.unwrap_or_default(),
        None => String::new(),
    }
}

fn power_i64(p: matrix_sdk::ruma::events::room::power_levels::UserPowerLevel) -> i64 {
    use matrix_sdk::ruma::events::room::power_levels::UserPowerLevel;
    match p {
        UserPowerLevel::Infinite => 100,
        UserPowerLevel::Int(i) => i64::from(i),
        _ => 0,
    }
}

/// Can `me` write `m.room.power_levels` in `room` (the reconciler's gate)?
async fn can_write_power_levels(room: &Room) -> bool {
    match room.power_levels().await {
        Ok(pl) => pl.user_can_send_state(room.own_user_id(), StateEventType::RoomPowerLevels),
        Err(_) => false,
    }
}

/// Joined child rooms of a space (from its active `m.space.child` events), in a stable order.
async fn child_ids(space: &Room) -> Vec<String> {
    let mut ids: Vec<String> = crate::nav::space_child_ids(space).await.into_iter().collect();
    ids.sort();
    ids
}

fn joined_child(client: &Client, id: &str) -> Option<Room> {
    let rid = RoomId::parse(id).ok()?;
    let room = client.get_room(&rid)?;
    (room.state() == RoomState::Joined).then_some(room)
}

/// `synced` for the Rooms tab: the derived power levels equal what the room has.
async fn room_synced(room: &Room, st: &RolesState, is_space: bool) -> bool {
    let Some(cur) = crate::appstate::get_state(room, "m.room.power_levels", "").await else {
        return true; // no state yet — nothing to compare (a just-created room)
    };
    roles_model::derive_power_levels(&cur, st, is_space) == cur
}

struct MemberRow {
    mxid: String,
    name: String,
    avatar: String,
    membership: &'static str,
    rank: i64,
    power: i64,
    roles: Vec<String>,
    top: Value,
    tag: Value,      // 10.4: highest custom role {id,name,color} | null — the name colour
    is_me: bool,
    can_kick: bool,
    can_ban: bool,
    can_edit_roles: bool,
    reason: String,
}

async fn build_snapshot(space_id: &str) -> Result<Snapshot, String> {
    let client = crate::session::client().ok_or_else(|| "Not signed in".to_string())?;
    let rid = RoomId::parse(space_id).map_err(|e| format!("Bad space id: {e}"))?;
    let space = client.get_room(&rid).ok_or_else(|| "That space isn't loaded".to_string())?;
    let me = space.own_user_id().to_string();
    let state = space.state();
    let joined = state == RoomState::Joined;

    let st = if joined { roles::load_state_room(&space).await } else { RolesState::default() };
    let active = st.active();
    let my: Perms = if joined {
        roles::effective_perms_with(&space, &me, &st, space_id.to_string()).await
    } else {
        Perms::none(space_id)
    };
    let my_rank = my.rank;

    // ---- members -------------------------------------------------------------------------
    let members = if joined {
        space
            .members(RoomMemberships::JOIN | RoomMemberships::INVITE | RoomMemberships::BAN)
            .await
            .map_err(|e| format!("Couldn't load members: {e}"))?
    } else {
        Vec::new()
    };
    let mut rows: Vec<MemberRow> = Vec::with_capacity(members.len());
    let (mut n_join, mut n_invite, mut n_ban) = (0usize, 0usize, 0usize);
    for m in &members {
        let membership = match m.membership() {
            MembershipState::Invite => "invite",
            MembershipState::Ban => "ban",
            MembershipState::Join => "join",
            _ => continue,
        };
        match membership {
            "join" => n_join += 1,
            "invite" => n_invite += 1,
            _ => n_ban += 1,
        }
        let mxid = m.user_id().to_string();
        let power = power_i64(m.power_level());
        let is_me = mxid == me;
        let (rank, role_ids, top) = if active {
            let rs = st.roles_of(&mxid);
            let ids: Vec<String> = rs.iter().map(|r| r.id.clone()).collect();
            let top = rs
                .first()
                .map(|r| json!({"id": r.id, "name": r.name, "color": r.color}))
                .unwrap_or(Value::Null);
            (st.rank_of(&mxid), ids, top)
        } else {
            let top = if power >= 100 {
                json!({"id": "pl", "name": "Admin", "color": ""})
            } else if power >= 50 {
                json!({"id": "pl", "name": "Moderator", "color": ""})
            } else {
                Value::Null
            };
            (power, Vec::new(), top)
        };
        let outranked = rank < my_rank;
        let avatar = match m.avatar_url() {
            Some(mxc) => crate::bridge::resolve_avatar(&client, mxc).await.unwrap_or_default(),
            None => String::new(),
        };
        rows.push(MemberRow {
            name: m.name().to_string(),
            avatar,
            membership,
            rank,
            power,
            roles: role_ids,
            tag: if active {
                st.tag_role_of(&mxid)
                    .map(|r| json!({"id": r.id, "name": r.name, "color": r.color}))
                    .unwrap_or(Value::Null)
            } else {
                Value::Null
            },
            top,
            is_me,
            can_kick: my.has("kick") && !is_me && membership != "ban" && outranked,
            can_ban: my.has("ban") && !is_me && outranked,
            can_edit_roles: active
                && my.has("manage_roles")
                && membership != "ban"
                && (is_me || outranked),
            reason: m.event().reason().unwrap_or("").to_string(),
            mxid,
        });
    }
    let section = |m: &str| match m {
        "join" => 0,
        "invite" => 1,
        _ => 2,
    };
    rows.sort_by(|a, b| {
        section(a.membership)
            .cmp(&section(b.membership))
            .then_with(|| b.rank.cmp(&a.rank))
            .then_with(|| b.power.cmp(&a.power))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.mxid.cmp(&b.mxid))
    });
    let joined_ids: BTreeSet<&str> =
        rows.iter().filter(|r| r.membership == "join").map(|r| r.mxid.as_str()).collect();
    let members_val: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "mxid": r.mxid, "name": r.name, "avatar": r.avatar, "membership": r.membership,
                "rank": r.rank, "roles": r.roles, "topRole": r.top, "tag": r.tag, "isMe": r.is_me,
                "canKick": r.can_kick, "canBan": r.can_ban, "canEditRoles": r.can_edit_roles,
                "reason": r.reason,
            })
        })
        .collect();

    // ---- roles -----------------------------------------------------------------------------
    let can_manage_roles = my.has("manage_roles");
    let roles_val: Vec<Value> = st
        .sorted_roles()
        .iter()
        .map(|r| {
            let is_everyone = r.id == EVERYONE;
            let count = if is_everyone {
                n_join
            } else {
                st.members
                    .iter()
                    .filter(|(m, ids)| joined_ids.contains(m.as_str()) && ids.contains(&r.id))
                    .count()
            };
            let editable = can_manage_roles && (my_rank >= 100 || r.rank < my_rank);
            let perms: Vec<&str> = r.permissions.iter().map(String::as_str).collect();
            json!({
                "id": r.id, "name": r.name, "color": r.color, "rank": r.rank,
                "hoist": r.hoist,
                "permissions": perms, "memberCount": count,
                "editable": editable, "deletable": editable && !is_everyone,
                "isEveryone": is_everyone,
            })
        })
        .collect();

    // ---- rooms -----------------------------------------------------------------------------
    let mut rooms_val: Vec<Value> = Vec::new();
    let mut out_of_sync = 0usize;
    let children = if joined { child_ids(&space).await } else { Vec::new() };
    for cid in &children {
        let Some(room) = joined_child(&client, cid) else {
            rooms_val.push(json!({
                "id": cid, "name": cid, "avatar": "", "joined": false, "synced": true, "canSync": false,
            }));
            continue;
        };
        let synced = if active { room_synced(&room, &st, false).await } else { true };
        let can_sync = active && can_write_power_levels(&room).await;
        if !synced {
            out_of_sync += 1;
        }
        rooms_val.push(json!({
            "id": cid,
            "name": room_display_name(&room).await,
            "avatar": room_avatar(&client, &room).await,
            "joined": true, "synced": synced, "canSync": can_sync,
        }));
    }

    // ---- summary ---------------------------------------------------------------------------
    let alias = space.canonical_alias().map(|a| a.to_string()).unwrap_or_default();
    let summary = json!({
        "id": space_id,
        "name": room_display_name(&space).await,
        "topic": space.topic().unwrap_or_default(),
        "avatar": room_avatar(&client, &space).await,
        "alias": alias,
        "link": permalink(space_id, &alias),
        "joinRule": join_rule_str(&space),
        "memberCount": if joined { n_join as u64 } else { space.joined_members_count() },
        "invitedCount": n_invite,
        "bannedCount": n_ban,
        "roomCount": children.len(),
        "encrypted": space.encryption_state().is_encrypted(),
        "membership": membership_str(state),
        "isMember": joined,
        "canManageSpace": my.has("manage_space"),
        "canManageRoles": can_manage_roles,
        "canManageRooms": my.has("manage_rooms"),
        "canInvite": my.has("invite"),
        "canKick": my.has("kick"),
        "canBan": my.has("ban"),
        "outOfSyncRooms": out_of_sync,
    });

    Ok(Snapshot {
        summary: summary.to_string(),
        members: Value::Array(members_val).to_string(),
        roles: Value::Array(roles_val).to_string(),
        my_perms: my.json(),
        rooms: Value::Array(rooms_val).to_string(),
        roles_active: active,
        my_rank: my_rank.clamp(0, i32::MAX as i64) as i32,
    })
}

// ---- actions --------------------------------------------------------------------------------

const LOCKOUT: &str = "That would lock you out of managing roles.";

/// What every action starts from: the joined space, who I am, the roles, my permissions.
struct Ctx {
    client: Client,
    space: Room,
    me: String,
    st: RolesState,
    my: Perms,
}

impl Ctx {
    async fn load(space_id: &str) -> Result<Ctx, String> {
        let client = crate::session::client().ok_or_else(|| "Not signed in".to_string())?;
        let rid = RoomId::parse(space_id).map_err(|e| format!("Bad space id: {e}"))?;
        let space = client.get_room(&rid).ok_or_else(|| "That space isn't loaded".to_string())?;
        if space.state() != RoomState::Joined {
            return Err("You're not a member of that space".to_string());
        }
        let me = space.own_user_id().to_string();
        let st = roles::load_state_room(&space).await;
        let my = roles::effective_perms_with(&space, &me, &st, space_id.to_string()).await;
        Ok(Ctx { client, space, me, st, my })
    }

    fn need(&self, perm: &str, what: &str) -> Result<(), String> {
        if self.my.has(perm) {
            Ok(())
        } else {
            let label = roles_model::PERMISSIONS
                .iter()
                .find(|p| p.key == perm)
                .map(|p| p.label)
                .unwrap_or(perm);
            Err(format!("You need the \"{label}\" permission to {what}"))
        }
    }

    fn need_active(&self) -> Result<(), String> {
        if self.st.active() {
            Ok(())
        } else {
            Err("Roles aren't set up in this space yet".to_string())
        }
    }

    /// A member's rank for the "who may act on whom" rule (roles, else the space's power level).
    async fn rank_of(&self, mxid: &str) -> i64 {
        if self.st.active() {
            self.st.rank_of(mxid)
        } else {
            roles::power_level_of(&self.space, mxid).await
        }
    }

    /// The homeserver's rule, applied first so the user gets a sentence instead of a 403.
    async fn must_outrank(&self, mxid: &str) -> Result<(), String> {
        if self.rank_of(mxid).await >= self.my.rank {
            let name = self.display(mxid).await;
            return Err(format!("{name} is ranked at or above you"));
        }
        Ok(())
    }

    async fn display(&self, mxid: &str) -> String {
        if let Ok(uid) = UserId::parse(mxid) {
            if let Ok(Some(m)) = self.space.get_member_no_sync(&uid).await {
                return m.name().to_string();
            }
            return uid.localpart().to_string();
        }
        mxid.to_string()
    }

    /// Joined child rooms of the space.
    async fn children(&self) -> Vec<Room> {
        let mut out = Vec::new();
        for cid in child_ids(&self.space).await {
            if let Some(r) = joined_child(&self.client, &cid) {
                out.push(r);
            }
        }
        out
    }

    /// The lock-out guards (§4): the acting user keeps `manage_roles`, and nobody demotes the
    /// last rank-100 member.
    fn guard_lockout(&self, next: &RolesState) -> Result<(), String> {
        if !next.has(&self.me, "manage_roles") {
            return Err(LOCKOUT.to_string());
        }
        if !self.st.members_at_or_above(100).is_empty() && next.members_at_or_above(100).is_empty() {
            return Err(LOCKOUT.to_string());
        }
        Ok(())
    }

    /// §4: write the derived power levels to the space and every joined child where allowed.
    /// Returns (written, failed).
    async fn reconcile(&self, st: &RolesState) -> (usize, usize) {
        let mut targets: Vec<(Room, bool)> = vec![(self.space.clone(), true)];
        for c in self.children().await {
            targets.push((c, false));
        }
        let (mut written, mut failed) = (0usize, 0usize);
        for (room, is_space) in targets {
            let Some(cur) = crate::appstate::get_state(&room, "m.room.power_levels", "").await else {
                continue;
            };
            let next = roles_model::derive_power_levels(&cur, st, is_space);
            if next == cur {
                continue;
            }
            if !can_write_power_levels(&room).await {
                eprintln!("[space] reconcile: no power-level rights in {}", room.room_id());
                failed += 1;
                continue;
            }
            match room.send_state_event_raw("m.room.power_levels", "", next).await {
                Ok(_) => written += 1,
                Err(e) => {
                    eprintln!("[space] reconcile {} failed: {e}", room.room_id());
                    failed += 1;
                }
            }
        }
        (written, failed)
    }
}

/// Wait (bounded) until the local store shows `ok(content)` for a state event we just wrote,
/// so the refresh that follows an action reflects it rather than the pre-write state.
async fn settle(room: &Room, event_type: &str, state_key: &str, ok: impl Fn(&Value) -> bool) {
    for _ in 0..16 {
        if let Some(v) = crate::appstate::get_state(room, event_type, state_key).await {
            if ok(&v) {
                return;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

/// The same for a membership change.
async fn settle_membership(room: &Room, uid: &UserId, want: &[MembershipState]) {
    for _ in 0..16 {
        let cur = room.get_member_no_sync(uid).await.ok().flatten().map(|m| m.membership().clone());
        match cur {
            Some(m) if want.contains(&m) => return,
            None if want.contains(&MembershipState::Leave) => return,
            _ => {}
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

async fn do_apply_name(space_id: &str, name: &str) -> Result<String, String> {
    let c = Ctx::load(space_id).await?;
    c.need("manage_space", "rename the space")?;
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("Give the space a name".to_string());
    }
    c.space.set_name(name.clone()).await.map_err(|e| format!("Couldn't rename the space: {e}"))?;
    settle(&c.space, "m.room.name", "", |v| v.get("name").and_then(Value::as_str) == Some(&name)).await;
    Ok("Name saved".to_string())
}

async fn do_apply_topic(space_id: &str, topic: &str) -> Result<String, String> {
    let c = Ctx::load(space_id).await?;
    c.need("manage_space", "change the topic")?;
    let topic = topic.trim().to_string();
    c.space.set_room_topic(&topic).await.map_err(|e| format!("Couldn't change the topic: {e}"))?;
    settle(&c.space, "m.room.topic", "", |v| {
        v.get("topic").and_then(Value::as_str).unwrap_or("") == topic
    })
    .await;
    Ok("Topic saved".to_string())
}

async fn do_apply_photo(space_id: &str, path: &str) -> Result<String, String> {
    let c = Ctx::load(space_id).await?;
    c.need("manage_space", "change the photo")?;
    let before = crate::appstate::get_state(&c.space, "m.room.avatar", "").await;
    crate::room::do_apply_room_photo(&c.client, space_id, path)
        .await
        .map_err(|e| format!("Couldn't change the photo: {e}"))?;
    settle(&c.space, "m.room.avatar", "", |v| Some(v) != before.as_ref()).await;
    Ok(if path.is_empty() { "Photo removed" } else { "Photo updated" }.to_string())
}

async fn do_apply_join_rule(space_id: &str, rule: &str) -> Result<String, String> {
    let c = Ctx::load(space_id).await?;
    c.need("manage_space", "change who can join")?;
    let (content, msg) = match rule {
        "public" => (json!({"join_rule": "public"}), "Anyone with the link can join now"),
        "invite" => (json!({"join_rule": "invite"}), "Invite only now"),
        other => return Err(format!("Unknown join rule \"{other}\"")),
    };
    c.space
        .send_state_event_raw("m.room.join_rules", "", content)
        .await
        .map_err(|e| format!("Couldn't change who can join: {e}"))?;
    settle(&c.space, "m.room.join_rules", "", |v| v.get("join_rule").and_then(Value::as_str) == Some(rule)).await;
    Ok(msg.to_string())
}

async fn do_invite(space_id: &str, mxid: &str) -> Result<String, String> {
    let c = Ctx::load(space_id).await?;
    c.need("invite", "invite people")?;
    let uid = UserId::parse(mxid.trim()).map_err(|_| format!("{mxid} doesn't look like a user id"))?;
    if let Ok(Some(m)) = c.space.get_member_no_sync(&uid).await {
        match m.membership() {
            MembershipState::Join => return Err(format!("{} is already a member", m.name())),
            MembershipState::Invite => return Err(format!("{} is already invited", m.name())),
            MembershipState::Ban => return Err(format!("{} is banned — unban them first", m.name())),
            _ => {}
        }
    }
    c.space.invite_user_by_id(&uid).await.map_err(|e| format!("Couldn't invite {}: {e}", uid.localpart()))?;
    settle_membership(&c.space, &uid, &[MembershipState::Invite, MembershipState::Join]).await;
    Ok(format!("Invited {}", c.display(uid.as_str()).await))
}

async fn do_search_users(space_id: &str, query: &str) -> String {
    let Some(client) = crate::session::client() else {
        return "[]".to_string();
    };
    let space = RoomId::parse(space_id).ok().and_then(|rid| client.get_room(&rid));
    let resp = match client.search_users(query, 20).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[space] user search failed: {e}");
            return "[]".to_string();
        }
    };
    let mut out = Vec::with_capacity(resp.results.len());
    for u in resp.results {
        let already = match &space {
            Some(s) => matches!(
                s.get_member_no_sync(&u.user_id).await.ok().flatten().map(|m| m.membership().clone()),
                Some(MembershipState::Join) | Some(MembershipState::Invite)
            ),
            None => false,
        };
        let avatar = match &u.avatar_url {
            Some(mxc) => crate::bridge::resolve_avatar(&client, mxc).await.unwrap_or_default(),
            None => String::new(),
        };
        out.push(json!({
            "mxid": u.user_id.to_string(),
            "name": u.display_name.clone().unwrap_or_else(|| u.user_id.localpart().to_string()),
            "avatar": avatar,
            "alreadyMember": already,
        }));
    }
    Value::Array(out).to_string()
}

#[derive(Clone, Copy, PartialEq)]
enum MemberAction {
    Kick,
    Ban,
    Unban,
}

/// Kick / ban / unban on the space, cascaded into every joined child room where the actor may
/// (§5.3: "Removed Elena from the space and 2 rooms").
async fn do_membership(space_id: &str, mxid: &str, reason: &str, action: MemberAction) -> Result<String, String> {
    let c = Ctx::load(space_id).await?;
    let (perm, what) = match action {
        MemberAction::Kick => ("kick", "remove people"),
        MemberAction::Ban => ("ban", "ban people"),
        MemberAction::Unban => ("ban", "unban people"),
    };
    c.need(perm, what)?;
    let uid = UserId::parse(mxid.trim()).map_err(|_| format!("{mxid} doesn't look like a user id"))?;
    if uid.as_str() == c.me {
        return Err("You can't do that to yourself — use Leave instead".to_string());
    }
    c.must_outrank(uid.as_str()).await?;
    let name = c.display(uid.as_str()).await;
    let reason = if reason.trim().is_empty() { None } else { Some(reason.trim()) };

    let membership_of = |room: &Room, uid: &UserId| {
        let room = room.clone();
        let uid = uid.to_owned();
        async move { room.get_member_no_sync(&uid).await.ok().flatten().map(|m| m.membership().clone()) }
    };

    // The space first — if that fails the action failed.
    let cur = membership_of(&c.space, &uid).await;
    match action {
        MemberAction::Kick => {
            // (left_words.rs: they may have left while the confirmation was open)
            if let Some(gone) = crate::left_words::gone_text(crate::left_words::LeftScope::Space, &name, cur.as_ref().map(|m| m.as_str()), false) {
                return Err(gone);
            }
            c.space.kick_user(&uid, reason).await.map_err(|e| format!("Couldn't remove {name}: {e}"))?;
        }
        MemberAction::Ban => {
            if matches!(cur, Some(MembershipState::Ban)) {
                return Err(format!("{name} is already banned"));
            }
            c.space.ban_user(&uid, reason).await.map_err(|e| format!("Couldn't ban {name}: {e}"))?;
        }
        MemberAction::Unban => {
            if !matches!(cur, Some(MembershipState::Ban)) {
                return Err(format!("{name} isn't banned"));
            }
            c.space.unban_user(&uid, reason).await.map_err(|e| format!("Couldn't unban {name}: {e}"))?;
        }
    }

    // Then every joined child room where we may and it applies — best effort.
    let mut rooms_done = 0usize;
    for child in c.children().await {
        let Ok(pl) = child.power_levels().await else { continue };
        let allowed = match action {
            MemberAction::Kick => pl.user_can_kick(child.own_user_id()),
            MemberAction::Ban | MemberAction::Unban => pl.user_can_ban(child.own_user_id()),
        };
        if !allowed {
            continue;
        }
        let m = membership_of(&child, &uid).await;
        let res = match action {
            MemberAction::Kick => {
                if !matches!(m, Some(MembershipState::Join) | Some(MembershipState::Invite)) {
                    continue;
                }
                child.kick_user(&uid, reason).await
            }
            MemberAction::Ban => {
                if matches!(m, Some(MembershipState::Ban)) {
                    continue;
                }
                child.ban_user(&uid, reason).await
            }
            MemberAction::Unban => {
                if !matches!(m, Some(MembershipState::Ban)) {
                    continue;
                }
                child.unban_user(&uid, reason).await
            }
        };
        match res {
            Ok(()) => rooms_done += 1,
            Err(e) => eprintln!("[space] {perm} {uid} in {} failed: {e}", child.room_id()),
        }
    }

    let want = match action {
        MemberAction::Kick => vec![MembershipState::Leave],
        MemberAction::Ban => vec![MembershipState::Ban],
        MemberAction::Unban => vec![MembershipState::Leave],
    };
    settle_membership(&c.space, &uid, &want).await;

    let verb = match action {
        MemberAction::Kick => "Removed",
        MemberAction::Ban => "Banned",
        MemberAction::Unban => "Unbanned",
    };
    let tail = if rooms_done > 0 { format!(" and {}", plural(rooms_done, "room", "rooms")) } else { String::new() };
    Ok(format!("{verb} {name} from the space{tail}"))
}

async fn write_member_roles(space: &Room, mxid: &str, roles: &[String]) -> Result<(), String> {
    space
        .send_state_event_raw(MEMBER_ROLES_TYPE, &roles_model::state_key_for(mxid), json!({ "roles": roles }))
        .await
        .map(|_| ())
        .map_err(|e| format!("Couldn't save roles: {e}"))
}

async fn write_role(space: &Room, id: &str, content: Value) -> Result<(), String> {
    space
        .send_state_event_raw(ROLE_TYPE, id, content)
        .await
        .map(|_| ())
        .map_err(|e| format!("Couldn't save the role: {e}"))
}

fn reconcile_tail(written: usize, failed: usize) -> String {
    let mut s = String::new();
    if written > 0 {
        s.push_str(&format!(" — synced to {}", plural(written, "room", "rooms")));
    }
    if failed > 0 {
        s.push_str(&format!(" ({} couldn't be synced)", plural(failed, "room", "rooms")));
    }
    s
}

async fn do_set_member_roles(space_id: &str, mxid: &str, roles_json: &str) -> Result<String, String> {
    let c = Ctx::load(space_id).await?;
    c.need_active()?;
    c.need("manage_roles", "change someone's roles")?;
    let uid = UserId::parse(mxid.trim()).map_err(|_| format!("{mxid} doesn't look like a user id"))?;
    let mxid = uid.as_str();
    let is_me = mxid == c.me;
    if !is_me {
        c.must_outrank(mxid).await?;
    }
    let ids: Vec<String> = serde_json::from_str::<Vec<String>>(roles_json)
        .map_err(|_| "Bad roles list".to_string())?
        .into_iter()
        .filter(|id| id != EVERYONE)
        .collect();
    let mut seen = BTreeSet::new();
    let mut clean: Vec<String> = Vec::new();
    for id in ids {
        let Some(role) = c.st.roles.get(&id) else {
            return Err("That role doesn't exist any more".to_string());
        };
        if role.rank > c.my.rank {
            return Err(format!("You can't give the \"{}\" role — it's ranked above you", role.name));
        }
        if seen.insert(id.clone()) {
            clean.push(id);
        }
    }
    let next = c.st.with_member(mxid, clean.clone());
    c.guard_lockout(&next)?;
    write_member_roles(&c.space, mxid, &clean).await?;
    let (written, failed) = c.reconcile(&next).await;
    let want = clean.clone();
    settle(&c.space, MEMBER_ROLES_TYPE, &roles_model::state_key_for(mxid), move |v| {
        let got: Vec<String> = v
            .get("roles")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default();
        got == want
    })
    .await;
    let name = if is_me { "you".to_string() } else { c.display(mxid).await };
    Ok(format!("Updated roles for {name}{}", reconcile_tail(written, failed)))
}

async fn do_save_role(space_id: &str, role_json: &str) -> Result<String, String> {
    let c = Ctx::load(space_id).await?;
    c.need_active()?;
    c.need("manage_roles", "edit roles")?;
    let v: Value = serde_json::from_str(role_json).map_err(|_| "Bad role".to_string())?;
    let id_in = v.get("id").and_then(Value::as_str).unwrap_or("").trim().to_string();
    let creating = id_in.is_empty();
    let id = if creating { roles_model::new_role_id() } else { id_in };
    let existing = c.st.roles.get(&id).cloned();
    if !creating && existing.is_none() {
        return Err("That role doesn't exist any more".to_string());
    }
    if let Some(ex) = &existing {
        if ex.rank >= c.my.rank && c.my.rank < 100 {
            return Err("You can't edit a role ranked at or above yours".to_string());
        }
    }
    let name = v.get("name").and_then(Value::as_str).unwrap_or("").trim().to_string();
    if name.is_empty() {
        return Err("Give the role a name".to_string());
    }
    let color = v.get("color").and_then(Value::as_str).unwrap_or("").trim().to_string();
    let rank = if id == EVERYONE {
        0
    } else {
        v.get("rank")
            .and_then(Value::as_i64)
            .or_else(|| existing.as_ref().map(|e| e.rank))
            .unwrap_or(0)
            .clamp(0, 100)
    };
    if rank > c.my.rank {
        return Err("You can't rank a role above yourself".to_string());
    }
    let permissions: BTreeSet<String> = v
        .get("permissions")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .filter(|k| roles_model::is_known_permission(k))
                .map(str::to_string)
                .collect()
        })
        .or_else(|| existing.as_ref().map(|e| e.permissions.clone()))
        .unwrap_or_default();
    let hoist = v
        .get("hoist")
        .and_then(Value::as_bool)
        .or_else(|| existing.as_ref().map(|e| e.hoist))
        .unwrap_or(id != EVERYONE);
    let role = Role { id: id.clone(), name: name.clone(), color, rank, permissions, hoist };
    let next = c.st.with_role(role.clone());
    c.guard_lockout(&next)?;
    write_role(&c.space, &id, role.to_content()).await?;
    let (written, failed) = c.reconcile(&next).await;
    let want = role.to_content();
    settle(&c.space, ROLE_TYPE, &id, move |v| *v == want).await;
    let verb = if creating { "Created" } else { "Saved" };
    Ok(format!("{verb} the \"{name}\" role{}", reconcile_tail(written, failed)))
}

async fn do_delete_role(space_id: &str, role_id: &str) -> Result<String, String> {
    let c = Ctx::load(space_id).await?;
    c.need_active()?;
    c.need("manage_roles", "delete roles")?;
    if role_id == EVERYONE {
        return Err("The Everyone role can't be deleted".to_string());
    }
    let Some(role) = c.st.roles.get(role_id).cloned() else {
        return Err("That role doesn't exist any more".to_string());
    };
    if role.rank >= c.my.rank && c.my.rank < 100 {
        return Err("You can't delete a role ranked at or above yours".to_string());
    }
    let next = c.st.without_role(role_id);
    c.guard_lockout(&next)?;
    write_role(&c.space, role_id, json!({})).await?;
    let (written, failed) = c.reconcile(&next).await;
    settle(&c.space, ROLE_TYPE, role_id, |v| v.as_object().map(|o| o.is_empty()).unwrap_or(true)).await;
    Ok(format!("Deleted the \"{}\" role{}", role.name, reconcile_tail(written, failed)))
}

/// "Set up roles": the default template, admins/mods migrated from the current power levels,
/// then the derived power levels written everywhere we may.
async fn do_setup_roles(space_id: &str) -> Result<String, String> {
    let c = Ctx::load(space_id).await?;
    if c.st.active() {
        return Ok("Roles are already set up".to_string());
    }
    if !can_write_power_levels(&c.space).await {
        return Err("Only an admin of the space can set up roles".to_string());
    }
    // Migration input: joined members' power levels (creators' "infinite" reads as 100, which
    // the raw `users` map would miss).
    let members = c
        .space
        .members(RoomMemberships::JOIN)
        .await
        .map_err(|e| format!("Couldn't load members: {e}"))?;
    let users: Vec<(String, i64)> =
        members.iter().map(|m| (m.user_id().to_string(), power_i64(m.power_level()))).collect();
    let assigned = roles_model::migrate_from_power_levels(&users);
    let mut next = RolesState::default();
    for role in roles_model::default_template() {
        write_role(&c.space, &role.id, role.to_content()).await?;
        next.roles.insert(role.id.clone(), role);
    }
    for (mxid, ids) in &assigned {
        write_member_roles(&c.space, mxid, ids).await?;
        next.members.insert(mxid.clone(), ids.clone());
    }
    let (written, failed) = c.reconcile(&next).await;
    settle(&c.space, ROLE_TYPE, EVERYONE, |v| v.get("name").is_some()).await;
    let admins = assigned.values().filter(|r| r.iter().any(|x| x == "admin")).count();
    let mods = assigned.values().filter(|r| r.iter().any(|x| x == "moderator")).count();
    Ok(format!(
        "Roles set up: {} and {}{}",
        plural(admins, "admin", "admins"),
        plural(mods, "moderator", "moderators"),
        reconcile_tail(written, failed)
    ))
}

async fn do_sync_rooms(space_id: &str) -> Result<String, String> {
    let c = Ctx::load(space_id).await?;
    c.need_active()?;
    c.need("manage_roles", "sync the rooms")?;
    let (written, failed) = c.reconcile(&c.st).await;
    // Let the last write land before the refresh recomputes `synced`.
    if written > 0 {
        tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    }
    Ok(match (written, failed) {
        (0, 0) => "Every room is already in sync".to_string(),
        (w, 0) => format!("Roles synced to {}", plural(w, "room", "rooms")),
        (w, f) => format!(
            "Roles synced to {}, {} couldn't be synced",
            plural(w, "room", "rooms"),
            plural(f, "room", "rooms")
        ),
    })
}

async fn do_leave(space_id: &str) -> Result<String, String> {
    let client = crate::session::client().ok_or_else(|| "Not signed in".to_string())?;
    let rid = RoomId::parse(space_id).map_err(|e| format!("Bad space id: {e}"))?;
    let space = client.get_room(&rid).ok_or_else(|| "That space isn't loaded".to_string())?;
    let name = room_display_name(&space).await;
    space.leave().await.map_err(|e| format!("Couldn't leave {name}: {e}"))?;
    Ok(format!("Left {name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permalink_is_share_link() {
        // the rule and its cases live in share_link.rs (shared vectors); here only that a space uses it
        let i = crate::share_link::LinkInput { room: "!abc:example.com", own: "example.com", web: "https://chat.example.com", ..Default::default() };
        assert_eq!(crate::share_link::share_link(&i), "https://chat.example.com/#/room/!abc%3Aexample.com");
    }

    #[test]
    fn plurals() {
        assert_eq!(plural(1, "room", "rooms"), "1 room");
        assert_eq!(plural(2, "room", "rooms"), "2 rooms");
        assert_eq!(reconcile_tail(0, 0), "");
        assert_eq!(reconcile_tail(3, 0), " — synced to 3 rooms");
        assert_eq!(reconcile_tail(1, 2), " — synced to 1 room (2 rooms couldn't be synced)");
    }
}
