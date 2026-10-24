//! Async role helpers over the SDK (RBAC, docs/RBAC.md §5.2). No Qt here — `SpaceController`
//! (space.rs), `CallController` and `RoomController` all sit on these.
//!
//! Roles live in the SPACE room's state (§3); a child room "inherits" them through its
//! `m.space.parent`. [`container_of`] resolves that parent, [`load_state`] reads the roles from
//! the local store, and [`effective_perms`] answers "what may this person do here" in BOTH
//! modes — roles when the container has them, the room's power levels otherwise — so every
//! permission-gated UI keeps working on a space that never ran "Set up roles".

use std::collections::BTreeSet;

use matrix_sdk::ruma::{RoomId, UserId};
use matrix_sdk::{Room, RoomState};
use serde_json::{json, Value};

use crate::roles_model::{self, RolesState, MEMBER_ROLES_TYPE, ROLE_TYPE};

pub struct Perms {
    pub rank: i64,
    pub perms: BTreeSet<String>,
    pub roles_active: bool,
    pub container: String,
}

impl Perms {
    pub fn has(&self, key: &str) -> bool {
        self.perms.contains(key)
    }

    /// `{"kick":true,...}` for every catalogue key.
    pub fn json(&self) -> String {
        roles_model::perms_json(&self.perms)
    }

    /// No permissions at all (room unknown / not signed in).
    pub fn none(room_id: &str) -> Perms {
        Perms { rank: 0, perms: BTreeSet::new(), roles_active: false, container: room_id.to_string() }
    }
}

fn room_by_id(room_id: &str) -> Option<Room> {
    let client = crate::session::client()?;
    let rid = RoomId::parse(room_id).ok()?;
    client.get_room(&rid)
}

/// The room that holds the roles for `room_id`: its parent space (`m.space.parent` with a
/// non-empty `via`, canonical first, that we have JOINED) or the room itself. A space is its
/// own container.
pub async fn container_of(room_id: &str) -> String {
    match room_by_id(room_id) {
        Some(room) => container_of_room(&room).await,
        None => room_id.to_string(),
    }
}

pub async fn container_of_room(room: &Room) -> String {
    let own = room.room_id().to_string();
    if room.is_space() {
        return own;
    }
    let Some(client) = crate::session::client() else {
        return own;
    };
    // (canonical, parent id) — a stable sort keeps the store's order among equals.
    let mut parents: Vec<(bool, String)> = Vec::new();
    for (parent_id, _sender, content) in crate::appstate::get_state_all(room, "m.space.parent").await {
        let via_ok = content
            .get("via")
            .and_then(Value::as_array)
            .map(|a| !a.is_empty())
            .unwrap_or(false);
        if !via_ok || parent_id.is_empty() {
            continue;
        }
        let canonical = content.get("canonical").and_then(Value::as_bool).unwrap_or(false);
        parents.push((canonical, parent_id));
    }
    parents.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, pid) in parents {
        let Ok(rid) = RoomId::parse(&pid) else { continue };
        if let Some(p) = client.get_room(&rid) {
            if p.state() == RoomState::Joined {
                return pid;
            }
        }
    }
    own
}

/// The roles of a container, from the local store (`appstate::get_state_all`). An unknown
/// room yields an empty, inactive state.
pub async fn load_state(container_id: &str) -> RolesState {
    match room_by_id(container_id) {
        Some(room) => load_state_room(&room).await,
        None => RolesState::default(),
    }
}

pub async fn load_state_room(room: &Room) -> RolesState {
    let roles: Vec<(String, Value)> = crate::appstate::get_state_all(room, ROLE_TYPE)
        .await
        .into_iter()
        .map(|(k, _, c)| (k, c))
        .collect();
    let members: Vec<(String, Value)> = crate::appstate::get_state_all(room, MEMBER_ROLES_TYPE)
        .await
        .into_iter()
        .map(|(k, _, c)| (k, c))
        .collect();
    RolesState::from_state(&roles, &members)
}

/// A member's power level in `room` as a plain number (room creators' "infinite" = 100).
pub async fn power_level_of(room: &Room, mxid: &str) -> i64 {
    use matrix_sdk::ruma::events::room::power_levels::UserPowerLevel;
    let Ok(uid) = UserId::parse(mxid) else { return 0 };
    let Ok(pl) = room.power_levels().await else { return 0 };
    match pl.for_user(&uid) {
        UserPowerLevel::Infinite => 100,
        UserPowerLevel::Int(i) => i64::from(i),
        _ => 0,
    }
}

/// What `mxid` may do in `room_id`: the container's roles when active, else the room's own
/// power levels through `perms_from_power_levels`.
pub async fn effective_perms(room_id: &str, mxid: &str) -> Perms {
    match room_by_id(room_id) {
        Some(room) => effective_perms_room(&room, mxid).await,
        None => Perms::none(room_id),
    }
}

pub async fn effective_perms_room(room: &Room, mxid: &str) -> Perms {
    let container = container_of_room(room).await;
    let st = if container == room.room_id().as_str() {
        load_state_room(room).await
    } else {
        load_state(&container).await
    };
    effective_perms_with(room, mxid, &st, container).await
}

/// `effective_perms` for a caller that already holds the container's state.
pub async fn effective_perms_with(room: &Room, mxid: &str, st: &RolesState, container: String) -> Perms {
    if st.active() {
        return Perms { rank: st.rank_of(mxid), perms: st.perms_of(mxid), roles_active: true, container };
    }
    let level = power_level_of(room, mxid).await;
    let pl = crate::appstate::get_state(room, "m.room.power_levels", "")
        .await
        .unwrap_or_else(|| json!({}));
    Perms { rank: level, perms: roles_model::perms_from_power_levels(level, &pl), roles_active: false, container }
}

/// The legacy power-level label (`Admin|Mod|Member`).
pub fn pl_label(power: i64) -> &'static str {
    if power >= 100 {
        "Admin"
    } else if power >= 50 {
        "Mod"
    } else {
        "Member"
    }
}

/// 10.4 (space tags): name + colour of the member's highest CUSTOM role, ("", "") when the
/// person has none (or the space has no roles). This is what colours a name and tags it.
pub fn tag_badge_from(st: &RolesState, mxid: &str) -> (String, String) {
    if st.active() {
        if let Some(r) = st.tag_role_of(mxid) {
            return (r.name.clone(), r.color.clone());
        }
    }
    (String::new(), String::new())
}

/// Role label + colour for a member: the top assigned role when roles are active, else
/// (`Admin|Mod|Member`, "") from the power level. A member with no assigned role in an
/// active space also gets the power-level label, so an unmigrated creator/bot still reads
/// "Admin" rather than "Member".
pub fn role_badge_from(st: &RolesState, mxid: &str, power: i64) -> (String, String) {
    if st.active() {
        if let Some(top) = st.roles_of(mxid).first() {
            return (top.name.clone(), top.color.clone());
        }
    }
    (pl_label(power).to_string(), String::new())
}

/// [`role_badge_from`] with the container looked up per call. RoomController's member loop
/// should load the state once and call `role_badge_from` per row instead.
pub async fn role_badge_for(room: &Room, mxid: &str, power: i64) -> (String, String) {
    let container = container_of_room(room).await;
    let st = load_state(&container).await;
    role_badge_from(&st, mxid, power)
}
