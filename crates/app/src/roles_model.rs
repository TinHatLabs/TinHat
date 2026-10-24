//! Roles + permissions pure model (RBAC, docs/RBAC.md §2–§4, §5.1).
//!
//! No SDK or Qt types: everything here is `serde_json` in, `serde_json` out, so it is unit-tested
//! in isolation. A space carries named **roles** (colour, rank 0–100, a set of permission keys)
//! and a per-member list of assigned roles, both as Matrix state in the SPACE room (§3). The
//! homeserver enforces nothing about roles directly — the client **derives**
//! `m.room.power_levels` from them ([`derive_power_levels`]) and writes that to the space and
//! every child room, so a modified client can never exceed what the roles grant.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

pub const ROLE_TYPE: &str = "dev.tinhat.role";
pub const MEMBER_ROLES_TYPE: &str = "dev.tinhat.member_roles";
pub const CALL_MOD_TYPE: &str = "dev.tinhat.call.moderation";
pub const EVERYONE: &str = "everyone";

/// Which layer makes a permission stick (§1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Enforcement {
    /// The homeserver, through the derived power levels.
    Matrix,
    /// The Tin Hat server (`/api/moderation/`), which reads the roles itself.
    Server,
    /// Tin Hat hides / blocks the action; another client could ignore it ("honour system").
    Client,
}

impl Enforcement {
    pub fn as_str(self) -> &'static str {
        match self {
            Enforcement::Matrix => "matrix",
            Enforcement::Server => "server",
            Enforcement::Client => "client",
        }
    }
}

pub struct PermDef {
    pub key: &'static str,
    pub label: &'static str,
    pub group: &'static str,
    pub description: &'static str,
    pub enforcement: Enforcement,
}

/// The v1 catalogue — exactly the table in §2, in that order. Keys are stable: never rename.
pub const PERMISSIONS: &[PermDef] = &[
    PermDef {
        key: "manage_space",
        label: "Manage space",
        group: "Space",
        description: "Change the name, topic, photo, who can join and the space link",
        enforcement: Enforcement::Matrix,
    },
    PermDef {
        key: "manage_roles",
        label: "Manage roles",
        group: "Space",
        description: "Create and edit roles and give them to people",
        enforcement: Enforcement::Matrix,
    },
    PermDef {
        key: "manage_rooms",
        label: "Manage rooms",
        group: "Space",
        description: "Create rooms in the space and add or remove rooms from it",
        enforcement: Enforcement::Matrix,
    },
    PermDef {
        key: "invite",
        label: "Invite people",
        group: "People",
        description: "Invite people to the space and its rooms",
        enforcement: Enforcement::Matrix,
    },
    PermDef {
        key: "kick",
        label: "Remove people",
        group: "People",
        description: "Remove someone from the space and its rooms",
        enforcement: Enforcement::Matrix,
    },
    PermDef {
        key: "ban",
        label: "Ban people",
        group: "People",
        description: "Ban and unban people",
        enforcement: Enforcement::Matrix,
    },
    PermDef {
        key: "manage_messages",
        label: "Manage messages",
        group: "Chat",
        description: "Delete anyone's messages",
        enforcement: Enforcement::Matrix,
    },
    PermDef {
        key: "pin_messages",
        label: "Pin messages",
        group: "Chat",
        description: "Pin and unpin messages",
        enforcement: Enforcement::Matrix,
    },
    PermDef {
        key: "mention_everyone",
        label: "Mention everyone",
        group: "Chat",
        description: "Use @room to notify everyone",
        enforcement: Enforcement::Matrix,
    },
    PermDef {
        key: "moderate_calls",
        label: "Moderate calls",
        group: "Calls",
        description: "Mute someone's mic for everyone, disconnect them from a call",
        enforcement: Enforcement::Server,
    },
    PermDef {
        key: "share_screen",
        label: "Share screen",
        group: "Calls",
        description: "Share a screen or window in calls",
        enforcement: Enforcement::Client,
    },
    PermDef {
        key: "use_soundboard",
        label: "Use the soundboard",
        group: "Calls",
        description: "Play sounds in calls",
        enforcement: Enforcement::Client,
    },
    PermDef {
        key: "manage_soundboard",
        label: "Manage the soundboard",
        group: "Calls",
        description: "Add and remove the space's shared sounds",
        enforcement: Enforcement::Matrix,
    },
    PermDef {
        key: "record_calls",
        label: "Record calls",
        group: "Calls",
        description: "Start and stop Memories recording",
        enforcement: Enforcement::Matrix,
    },
    PermDef {
        key: "manage_calendar",
        label: "Manage the calendar",
        group: "Squad",
        description: "Edit or cancel anyone's events",
        enforcement: Enforcement::Client,
    },
    PermDef {
        key: "manage_games",
        label: "Manage game nights",
        group: "Squad",
        description: "Change game-night rules, credits and the reel",
        enforcement: Enforcement::Matrix,
    },
    PermDef {
        key: "manage_watchparty",
        label: "Run Media Parties",
        group: "Squad",
        description: "Take over control of any Media Party",
        enforcement: Enforcement::Client,
    },
    // 11.1 (the Game servers widget, docs/GAMESERVERS.md): the Tin Hat server (its game-servers API) reads the
    // roles itself on every power request; no Matrix mapping. Appended: keys and indices above never move.
    PermDef {
        key: "manage_game_servers",
        label: "Manage game servers",
        group: "Squad",
        description: "Start, stop and restart the game servers",
        enforcement: Enforcement::Server,
    },
    // widgets W1 (the Sticky note widget, WIDGETS.md §14): the space's `dev.tinhat.note` state. Appended after
    // 11.1's manage_game_servers: keys and indices above never move (the Roles editors group by `group`, so it lists
    // under Space).
    PermDef {
        key: "edit_space_note",
        label: "Edit the space note",
        group: "Space",
        description: "Write and change the note on the space's Sticky note widget",
        enforcement: Enforcement::Matrix,
    },
];

/// Top-level `m.room.power_levels` keys a permission maps to (§2 "Matrix mapping").
const TOP_LEVEL_MAP: &[(&str, &str)] =
    &[("invite", "invite"), ("kick", "kick"), ("ban", "ban"), ("manage_messages", "redact")];

/// `events[<type>]` entries a permission maps to. `m.space.child` is only written into the
/// space itself and `m.space.parent` only into child rooms (see [`derive_power_levels`]).
const EVENT_MAP: &[(&str, &str)] = &[
    ("manage_space", "m.room.name"),
    ("manage_space", "m.room.topic"),
    ("manage_space", "m.room.avatar"),
    ("manage_space", "m.room.join_rules"),
    ("manage_space", "m.room.canonical_alias"),
    // NOT history_visibility / guest_access: Tin Hat has no UI for either, and the first live
    // "Set up roles" (Tin Hat Society, 2026-09-18) lowered history_visibility from Element's
    // admin-only 100 to the Moderator rank. Untouched keys keep whatever the room already had.
    ("manage_roles", "m.room.power_levels"),
    ("manage_roles", ROLE_TYPE),
    ("manage_roles", MEMBER_ROLES_TYPE),
    ("manage_rooms", "m.space.child"),
    ("manage_rooms", "m.space.parent"),
    ("pin_messages", "m.room.pinned_events"),
    ("moderate_calls", CALL_MOD_TYPE),
    ("manage_soundboard", "dev.tinhat.soundboard"),
    ("record_calls", "dev.tinhat.record"),
    ("manage_games", "dev.tinhat.games.settings"),
    ("manage_games", "dev.tinhat.games.adjust"),
    ("edit_space_note", "dev.tinhat.note"),
];

/// Every `events[<type>]` entry the reconciler owns. `appstate::ensure_writable` must skip
/// these when the room's container has roles active, or it would fight the reconciler.
const MANAGED_EVENT_TYPES: &[&str] = &[
    "m.room.name",
    "m.room.topic",
    "m.room.avatar",
    "m.room.join_rules",
    "m.room.canonical_alias",
    "m.room.power_levels",
    ROLE_TYPE,
    MEMBER_ROLES_TYPE,
    "m.space.child",
    "m.space.parent",
    "m.room.pinned_events",
    CALL_MOD_TYPE,
    "dev.tinhat.soundboard",
    "dev.tinhat.record",
    "dev.tinhat.games.settings",
    "dev.tinhat.games.adjust",
    "dev.tinhat.note",
];

pub fn managed_event_types() -> &'static [&'static str] {
    MANAGED_EVENT_TYPES
}

/// `[{key,label,group,description,enforcement}]` for the Roles editor.
pub fn permission_catalog_json() -> String {
    let arr: Vec<Value> = PERMISSIONS
        .iter()
        .map(|p| {
            json!({
                "key": p.key,
                "label": p.label,
                "group": p.group,
                "description": p.description,
                "enforcement": p.enforcement.as_str(),
            })
        })
        .collect();
    Value::Array(arr).to_string()
}

pub fn all_permission_keys() -> BTreeSet<String> {
    PERMISSIONS.iter().map(|p| p.key.to_string()).collect()
}

pub fn is_known_permission(key: &str) -> bool {
    PERMISSIONS.iter().any(|p| p.key == key)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Role {
    pub id: String,
    pub name: String,
    pub color: String,
    pub rank: i64,
    pub permissions: BTreeSet<String>,
    pub hoist: bool,
}

impl Role {
    /// The `dev.tinhat.role` event content for this role (§3).
    pub fn to_content(&self) -> Value {
        let perms: Vec<&str> = self.permissions.iter().map(String::as_str).collect();
        json!({
            "name": self.name,
            "color": self.color,
            "rank": self.rank,
            "permissions": perms,
            "hoist": self.hoist,
        })
    }

    /// Parse one role event. `None` for a deleted role (empty content) or garbage.
    pub fn from_content(id: &str, content: &Value) -> Option<Role> {
        let obj = content.as_object()?;
        if obj.is_empty() {
            return None;
        }
        let name = obj
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(id)
            .to_string();
        let color = obj.get("color").and_then(Value::as_str).unwrap_or("").trim().to_string();
        let rank = if id == EVERYONE {
            0
        } else {
            obj.get("rank").and_then(Value::as_i64).unwrap_or(0).clamp(0, 100)
        };
        let permissions = obj
            .get("permissions")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default();
        let hoist = obj.get("hoist").and_then(Value::as_bool).unwrap_or(false);
        Some(Role { id: id.to_string(), name, color, rank, permissions, hoist })
    }
}

/// The roles of one space: `roles` by id, `members` by **full mxid** (with `@`) → assigned
/// role ids. A member with a record but an empty list is still "managed" (rank 0), which is
/// what lets removing someone's last role demote them in the derived power levels.
#[derive(Clone, Debug, Default)]
pub struct RolesState {
    pub roles: BTreeMap<String, Role>,
    pub members: BTreeMap<String, Vec<String>>,
}

impl RolesState {
    /// Build from raw state: `roles` = `(state_key = role id, content)`,
    /// `members` = `(state_key = mxid minus '@', content)`.
    pub fn from_state(roles: &[(String, Value)], members: &[(String, Value)]) -> Self {
        let mut st = RolesState::default();
        for (id, content) in roles {
            if let Some(r) = Role::from_content(id, content) {
                st.roles.insert(id.clone(), r);
            }
        }
        for (key, content) in members {
            if key.is_empty() {
                continue;
            }
            let mxid = mxid_from_state_key(key);
            let ids: Vec<String> = content
                .get("roles")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                .unwrap_or_default();
            st.members.insert(mxid, ids);
        }
        st
    }

    /// Roles are active in a space iff the `everyone` role exists (§3).
    pub fn active(&self) -> bool {
        self.roles.contains_key(EVERYONE)
    }

    /// 10.4 (space tags): the role that shows ON a person — name colour + tag — is their
    /// highest CUSTOM role. Admin / Moderator / Everyone never colour a name: the tags are for
    /// the fun, empty roles people make up.
    pub fn tag_role_of(&self, mxid: &str) -> Option<&Role> {
        self.roles_of(mxid).into_iter().find(|r| !is_builtin(&r.id))
    }

    /// Max rank of the member's assigned roles, 0 with none.
    pub fn rank_of(&self, mxid: &str) -> i64 {
        self.roles_of(mxid).first().map(|r| r.rank).unwrap_or(0)
    }

    /// Union of the member's roles' permissions plus `everyone`.
    pub fn perms_of(&self, mxid: &str) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        if let Some(e) = self.roles.get(EVERYONE) {
            out.extend(e.permissions.iter().cloned());
        }
        for r in self.roles_of(mxid) {
            out.extend(r.permissions.iter().cloned());
        }
        out
    }

    pub fn has(&self, mxid: &str, perm: &str) -> bool {
        self.perms_of(mxid).contains(perm)
    }

    /// The member's assigned roles that exist, rank desc (ties by id so the order is stable).
    pub fn roles_of(&self, mxid: &str) -> Vec<&Role> {
        let mut out: Vec<&Role> = self
            .members
            .get(mxid)
            .map(|ids| ids.iter().filter_map(|id| self.roles.get(id)).collect())
            .unwrap_or_default();
        out.sort_by(|a, b| b.rank.cmp(&a.rank).then_with(|| a.id.cmp(&b.id)));
        out.dedup_by(|a, b| a.id == b.id);
        out
    }

    /// Every role, rank desc, `everyone` last (ties by name, then id).
    pub fn sorted_roles(&self) -> Vec<&Role> {
        let mut out: Vec<&Role> = self.roles.values().collect();
        out.sort_by(|a, b| {
            let ae = a.id == EVERYONE;
            let be = b.id == EVERYONE;
            ae.cmp(&be)
                .then_with(|| b.rank.cmp(&a.rank))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                .then_with(|| a.id.cmp(&b.id))
        });
        out
    }

    /// Lowest rank that holds `perm`, or 100 when no role does (§4 rule 3).
    pub fn threshold(&self, perm: &str) -> i64 {
        self.roles
            .values()
            .filter(|r| r.permissions.contains(perm))
            .map(|r| r.rank)
            .min()
            .unwrap_or(100)
    }

    /// Members (with a record) whose rank is `rank` or more.
    pub fn members_at_or_above(&self, rank: i64) -> Vec<String> {
        self.members.keys().filter(|m| self.rank_of(m) >= rank).cloned().collect()
    }

    /// A copy with one role replaced/added (for "would this change lock me out?" checks).
    pub fn with_role(&self, role: Role) -> RolesState {
        let mut st = self.clone();
        st.roles.insert(role.id.clone(), role);
        st
    }

    /// A copy without a role.
    pub fn without_role(&self, id: &str) -> RolesState {
        let mut st = self.clone();
        st.roles.remove(id);
        st
    }

    /// A copy with one member's assignment replaced.
    pub fn with_member(&self, mxid: &str, roles: Vec<String>) -> RolesState {
        let mut st = self.clone();
        st.members.insert(mxid.to_string(), roles);
        st
    }
}

/// Per-member state keys are the mxid WITHOUT its leading `@` (a state event whose state_key
/// starts with `@` may only be sent by that user — §3).
pub fn state_key_for(mxid: &str) -> String {
    mxid.strip_prefix('@').unwrap_or(mxid).to_string()
}

pub fn mxid_from_state_key(key: &str) -> String {
    if key.starts_with('@') {
        key.to_string()
    } else {
        format!("@{key}")
    }
}

fn perms(keys: &[&str]) -> BTreeSet<String> {
    keys.iter().map(|k| k.to_string()).collect()
}

/// The three roles "Set up roles" creates (§2 "Default roles").
pub fn default_template() -> Vec<Role> {
    vec![
        Role {
            id: "admin".into(),
            name: "Admin".into(),
            color: "#e8b923".into(),
            rank: 100,
            permissions: all_permission_keys(),
            hoist: true,
        },
        Role {
            id: "moderator".into(),
            name: "Moderator".into(),
            color: "#4da3ff".into(),
            rank: 50,
            // manage_space + manage_rooms are here so "Set up roles" takes NOTHING away from an
            // existing moderator: a stock Matrix room lets power 50 rename, re-topic, re-photo
            // and attach rooms (state_default 50). An admin can strip them in the Roles tab.
            permissions: perms(&[
                "invite",
                "kick",
                "ban",
                "manage_space",
                "manage_rooms",
                "manage_messages",
                "pin_messages",
                "mention_everyone",
                "moderate_calls",
                "share_screen",
                "use_soundboard",
                "manage_soundboard",
                "record_calls",
                "manage_calendar",
                "manage_games",
                "manage_watchparty",
                "edit_space_note",
            ]),
            hoist: true,
        },
        Role {
            id: EVERYONE.into(),
            name: "Everyone".into(),
            color: "#8a8f98".into(),
            rank: 0,
            // widgets W1: the space's note is the friend group's (2026-09-28: everyone may edit it)
            permissions: perms(&["invite", "share_screen", "use_soundboard", "record_calls", "edit_space_note"]),
            hoist: false,
        },
    ]
}

/// Migration from raw power levels: ≥ 100 → `admin`, ≥ 50 → `moderator`, else no record.
/// Keys are full mxids.
pub fn migrate_from_power_levels(users: &[(String, i64)]) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    for (mxid, level) in users {
        if *level >= 100 {
            out.insert(mxid.clone(), vec!["admin".to_string()]);
        } else if *level >= 50 {
            out.insert(mxid.clone(), vec!["moderator".to_string()]);
        }
    }
    out
}

fn pl_i64(pl: &Value, key: &str, default: i64) -> i64 {
    pl.get(key).and_then(Value::as_i64).unwrap_or(default)
}

fn pl_event(pl: &Value, event_type: &str, default: i64) -> i64 {
    pl.get("events").and_then(|e| e.get(event_type)).and_then(Value::as_i64).unwrap_or(default)
}

/// §4: the power-level content that enforces `st`. Starts from a deep copy of `current`,
/// never drops keys it does not understand, and is deterministic (same input → same output),
/// so callers compare before writing and skip the write when nothing changed.
pub fn derive_power_levels(current: &Value, st: &RolesState, is_space: bool) -> Value {
    let mut out = if current.is_object() { current.clone() } else { json!({}) };
    let users_default = pl_i64(&out, "users_default", 0);

    // Rule 2: managed members get users[mxid] = rank; unmanaged mxids are left alone.
    if !st.members.is_empty() {
        if !out.get("users").map(Value::is_object).unwrap_or(false) {
            out["users"] = json!({});
        }
        let users = out.get_mut("users").and_then(Value::as_object_mut).expect("users object");
        for mxid in st.members.keys() {
            let rank = st.rank_of(mxid);
            // No roles (rank 0) = the room's baseline whatever `users_default` is, so
            // stripping someone's last role never leaves them BELOW a fresh member.
            if rank == 0 || rank == users_default {
                users.remove(mxid);
            } else {
                users.insert(mxid.clone(), Value::from(rank));
            }
        }
    }

    // Rule 3: thresholds.
    for (perm, key) in TOP_LEVEL_MAP {
        out[*key] = Value::from(st.threshold(perm));
    }
    if !out.get("notifications").map(Value::is_object).unwrap_or(false) {
        out["notifications"] = json!({});
    }
    out["notifications"]["room"] = Value::from(st.threshold("mention_everyone"));
    if !out.get("events").map(Value::is_object).unwrap_or(false) {
        out["events"] = json!({});
    }
    for (perm, event_type) in EVENT_MAP {
        match *event_type {
            "m.space.child" if !is_space => continue,
            "m.space.parent" if is_space => continue,
            _ => {}
        }
        out["events"][*event_type] = Value::from(st.threshold(perm));
    }
    out
}

/// `"r-" + 8 hex` for a new custom role.
/// The template roles: never a name tag (see `RolesState::tag_role_of`).
pub fn is_builtin(id: &str) -> bool {
    matches!(id, "admin" | "moderator" | EVERYONE)
}

pub fn new_role_id() -> String {
    let n: u32 = rand::random();
    format!("r-{n:08x}")
}

/// Effective permissions when roles are NOT active, derived from the room's power levels so
/// every permission-gated UI works in both modes. Client-enforced permissions are true for
/// everyone (there is nothing to derive them from).
pub fn perms_from_power_levels(my_level: i64, pl: &Value) -> BTreeSet<String> {
    let state_default = pl_i64(pl, "state_default", 50);
    let notif_room = pl
        .get("notifications")
        .and_then(|n| n.get("room"))
        .and_then(Value::as_i64)
        .unwrap_or(50);
    let mut out = BTreeSet::new();
    let mut grant = |key: &str, needed: i64| {
        if my_level >= needed {
            out.insert(key.to_string());
        }
    };
    grant("kick", pl_i64(pl, "kick", 50));
    grant("ban", pl_i64(pl, "ban", 50));
    grant("invite", pl_i64(pl, "invite", 0));
    grant("manage_messages", pl_i64(pl, "redact", 50));
    grant("mention_everyone", notif_room);
    grant("pin_messages", pl_event(pl, "m.room.pinned_events", state_default));
    grant("moderate_calls", pl_event(pl, CALL_MOD_TYPE, 50));
    grant("manage_space", pl_event(pl, "m.room.name", state_default));
    grant("manage_roles", pl_event(pl, "m.room.power_levels", state_default));
    grant("manage_rooms", pl_event(pl, "m.space.child", state_default));
    grant("manage_soundboard", pl_event(pl, "dev.tinhat.soundboard", state_default));
    grant("record_calls", pl_event(pl, "dev.tinhat.record", state_default));
    grant("manage_games", pl_event(pl, "dev.tinhat.games.settings", state_default));
    // no Matrix mapping: a space without roles gives it to its owner rank (what the Tin Hat server checks too)
    grant("manage_game_servers", 100);
    grant("edit_space_note", pl_event(pl, "dev.tinhat.note", state_default));
    for p in PERMISSIONS.iter().filter(|p| p.enforcement == Enforcement::Client) {
        out.insert(p.key.to_string());
    }
    out
}

/// `{"<key>":bool,...}` for every catalogue key.
pub fn perms_json(perms: &BTreeSet<String>) -> String {
    let mut m = serde_json::Map::new();
    for p in PERMISSIONS {
        m.insert(p.key.to_string(), Value::Bool(perms.contains(p.key)));
    }
    Value::Object(m).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn template_state() -> RolesState {
        let roles: Vec<(String, Value)> =
            default_template().into_iter().map(|r| (r.id.clone(), r.to_content())).collect();
        let members = vec![
            ("blake:example.com".to_string(), json!({"roles": ["admin"]})),
            ("elena:example.com".to_string(), json!({"roles": ["moderator"]})),
            ("zane:example.com".to_string(), json!({"roles": []})),
        ];
        RolesState::from_state(&roles, &members)
    }

    /// What Synapse hands a freshly created private room (plus the members we know).
    fn stock_pl() -> Value {
        json!({
            "users": {"@blake:example.com": 100, "@elena:example.com": 50, "@bot:example.com": 100},
            "users_default": 0,
            "events": {
                "m.room.name": 50, "m.room.power_levels": 100, "m.room.history_visibility": 100,
                "m.room.canonical_alias": 50, "m.room.avatar": 50, "m.room.tombstone": 100,
                "m.room.server_acl": 100, "m.room.encryption": 100
            },
            "events_default": 0,
            "state_default": 50,
            "ban": 50, "kick": 50, "redact": 50, "invite": 0,
            "historical": 100,
            "notifications": {"room": 50}
        })
    }

    #[test]
    fn catalogue_matches_the_contract_table() {
        assert_eq!(PERMISSIONS.len(), 19);
        assert_eq!(PERMISSIONS[0].key, "manage_space");
        assert_eq!(PERMISSIONS[16].key, "manage_watchparty");
        assert_eq!(PERMISSIONS[17].key, "manage_game_servers");
        assert_eq!(PERMISSIONS[17].enforcement, Enforcement::Server);
        let v: Value = serde_json::from_str(&permission_catalog_json()).unwrap();
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 19);
        assert_eq!(arr[17]["label"], "Manage game servers");
        assert_eq!(arr[17]["description"], "Start, stop and restart the game servers");
        assert_eq!(PERMISSIONS[18].key, "edit_space_note");
        assert_eq!(arr[18]["key"], "edit_space_note");
        assert_eq!(arr[9]["key"], "moderate_calls");
        assert_eq!(arr[9]["enforcement"], "server");
        assert_eq!(arr[10]["enforcement"], "client");
        assert_eq!(arr[0]["enforcement"], "matrix");
        assert_eq!(arr[0]["group"], "Space");
        // Every mapped permission is in the catalogue and every mapping type is managed.
        for (perm, ty) in EVENT_MAP {
            assert!(is_known_permission(perm), "{perm}");
            assert!(managed_event_types().contains(ty), "{ty}");
        }
        for (perm, _) in TOP_LEVEL_MAP {
            assert!(is_known_permission(perm), "{perm}");
        }
    }

    #[test]
    fn role_roundtrip() {
        let r = Role {
            id: "r-0badf00d".into(),
            name: "Streamer".into(),
            color: "#ff5d73".into(),
            rank: 30,
            permissions: perms(&["share_screen", "record_calls"]),
            hoist: true,
        };
        let back = Role::from_content(&r.id, &r.to_content()).unwrap();
        assert_eq!(back, r);
        // Deleted / garbage content parses to nothing.
        assert!(Role::from_content("x", &json!({})).is_none());
        assert!(Role::from_content("x", &json!("nope")).is_none());
        // everyone is pinned to rank 0, unknown ranks clamp.
        let e = Role::from_content(EVERYONE, &json!({"name":"Everyone","rank":90})).unwrap();
        assert_eq!(e.rank, 0);
        let hi = Role::from_content("r-1", &json!({"name":"Hi","rank":900})).unwrap();
        assert_eq!(hi.rank, 100);
        // A nameless role falls back to its id.
        let anon = Role::from_content("r-2", &json!({"rank": 5})).unwrap();
        assert_eq!(anon.name, "r-2");
    }

    #[test]
    fn state_parse_rank_and_perm_union() {
        let st = template_state();
        assert!(st.active());
        assert_eq!(st.roles.len(), 3);
        assert_eq!(st.rank_of("@blake:example.com"), 100);
        assert_eq!(st.rank_of("@elena:example.com"), 50);
        assert_eq!(st.rank_of("@zane:example.com"), 0);
        assert_eq!(st.rank_of("@nobody:example.com"), 0);
        // Everyone's baseline reaches people with no roles at all.
        assert!(st.has("@zane:example.com", "invite"));
        assert!(st.has("@nobody:example.com", "use_soundboard"));
        assert!(!st.has("@zane:example.com", "kick"));
        assert!(st.has("@elena:example.com", "kick"));
        assert!(!st.has("@elena:example.com", "manage_roles"));
        assert!(st.has("@blake:example.com", "manage_roles"));
        // Union: a mod who also holds a custom role gets both sets.
        let st2 = st.with_role(Role {
            id: "r-1".into(),
            name: "Planner".into(),
            color: "".into(),
            rank: 10,
            permissions: perms(&["manage_calendar", "manage_space"]),
            hoist: false,
        });
        let st2 = st2.with_member("@zane:example.com", vec!["r-1".into()]);
        assert!(st2.has("@zane:example.com", "manage_space"));
        assert!(st2.has("@zane:example.com", "invite"));
        assert_eq!(st2.rank_of("@zane:example.com"), 10);
        // Unknown role ids are ignored, duplicates collapse.
        let st3 = st.with_member(
            "@zane:example.com",
            vec!["ghost".into(), "moderator".into(), "moderator".into()],
        );
        assert_eq!(st3.roles_of("@zane:example.com").len(), 1);
        assert_eq!(st3.rank_of("@zane:example.com"), 50);
    }

    #[test]
    fn sorted_roles_rank_desc_everyone_last() {
        let st = template_state().with_role(Role {
            id: "r-9".into(),
            name: "Zeta".into(),
            color: "".into(),
            rank: 0,
            permissions: BTreeSet::new(),
            hoist: false,
        });
        let ids: Vec<&str> = st.sorted_roles().iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["admin", "moderator", "r-9", "everyone"]);
        let st = st.with_member("@x:s", vec!["moderator".into(), "admin".into()]);
        let ids: Vec<&str> = st.roles_of("@x:s").iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["admin", "moderator"]);
    }

    #[test]
    fn inactive_without_everyone() {
        let roles = vec![("admin".to_string(), default_template()[0].to_content())];
        let st = RolesState::from_state(&roles, &[]);
        assert!(!st.active());
        // Deleted everyone (empty content) = inactive too.
        let roles = vec![
            ("admin".to_string(), default_template()[0].to_content()),
            (EVERYONE.to_string(), json!({})),
        ];
        assert!(!RolesState::from_state(&roles, &[]).active());
    }

    #[test]
    fn state_key_roundtrip() {
        assert_eq!(state_key_for("@logan:example.com"), "logan:example.com");
        assert_eq!(state_key_for("logan:example.com"), "logan:example.com");
        assert_eq!(mxid_from_state_key("logan:example.com"), "@logan:example.com");
        assert_eq!(mxid_from_state_key("@logan:example.com"), "@logan:example.com");
        let m = "@a:b.c";
        assert_eq!(mxid_from_state_key(&state_key_for(m)), m);
    }

    #[test]
    fn migration_thresholds() {
        let users = vec![
            ("@a:s".to_string(), 100),
            ("@b:s".to_string(), 150),
            ("@c:s".to_string(), 50),
            ("@d:s".to_string(), 99),
            ("@e:s".to_string(), 49),
            ("@f:s".to_string(), 0),
        ];
        let m = migrate_from_power_levels(&users);
        assert_eq!(m.get("@a:s").unwrap(), &vec!["admin".to_string()]);
        assert_eq!(m.get("@b:s").unwrap(), &vec!["admin".to_string()]);
        assert_eq!(m.get("@c:s").unwrap(), &vec!["moderator".to_string()]);
        assert_eq!(m.get("@d:s").unwrap(), &vec!["moderator".to_string()]);
        assert!(!m.contains_key("@e:s"));
        assert!(!m.contains_key("@f:s"));
    }

    #[test]
    fn min_rank_threshold() {
        let st = template_state();
        assert_eq!(st.threshold("kick"), 50);
        assert_eq!(st.threshold("invite"), 0);
        assert_eq!(st.threshold("manage_roles"), 100);
        // Nobody holds it → 100.
        let mut st2 = st.clone();
        for r in st2.roles.values_mut() {
            r.permissions.remove("ban");
        }
        assert_eq!(st2.threshold("ban"), 100);
        // The lowest holder wins even when a higher role also has it.
        let st3 = st.with_role(Role {
            id: "r-low".into(),
            name: "Low".into(),
            color: "".into(),
            rank: 10,
            permissions: perms(&["manage_roles"]),
            hoist: false,
        });
        assert_eq!(st3.threshold("manage_roles"), 10);
    }

    #[test]
    fn template_on_stock_pl_keeps_the_moderation_thresholds() {
        let st = template_state();
        let cur = stock_pl();
        let out = derive_power_levels(&cur, &st, true);
        for k in ["kick", "ban", "invite", "redact"] {
            assert_eq!(out[k], cur[k], "{k} changed");
        }
        assert_eq!(out["notifications"]["room"], cur["notifications"]["room"]);
        assert_eq!(out["events"]["m.room.power_levels"], 100);
        // Pinning + editing the shared board become Moderator (§2 notes both).
        assert_eq!(out["events"]["m.room.pinned_events"], 50);
        assert_eq!(out["events"]["dev.tinhat.soundboard"], 50);
        // widgets W1: the default template gives everyone the space note → any member (rank 0) may write it
        assert_eq!(out["events"]["dev.tinhat.note"], 0);
        // Moderator holds manage_space/manage_rooms in the template precisely so these stay at
        // the stock 50 — "Set up roles" must not take renaming away from an existing mod.
        assert_eq!(out["events"]["m.room.name"], 50);
        assert_eq!(out["events"]["m.room.topic"], 50);
        assert_eq!(out["events"]["m.room.canonical_alias"], 50);
        assert_eq!(out["events"]["m.space.child"], 50);
        // Unmanaged keys survive untouched.
        assert_eq!(out["events"]["m.room.tombstone"], 100);
        assert_eq!(out["events"]["m.room.encryption"], 100);
        assert_eq!(out["historical"], 100);
        assert_eq!(out["state_default"], 50);
        assert_eq!(out["events_default"], 0);
        assert_eq!(out["users_default"], 0);
    }

    #[test]
    fn users_managed_vs_untouched_and_space_only_keys() {
        let st = template_state();
        let cur = stock_pl();
        let out = derive_power_levels(&cur, &st, true);
        assert_eq!(out["users"]["@blake:example.com"], 100);
        assert_eq!(out["users"]["@elena:example.com"], 50);
        // The bot has no record → untouched. Zane has an empty record → removed (rank 0).
        assert_eq!(out["users"]["@bot:example.com"], 100);
        assert!(out["users"].get("@zane:example.com").is_none());
        // Demotion removes the key rather than writing 0.
        let cur2 = json!({"users": {"@zane:example.com": 50}});
        let out2 = derive_power_levels(&cur2, &st, true);
        assert!(out2["users"].get("@zane:example.com").is_none());
        // m.space.child only in the space, m.space.parent only in children.
        assert_eq!(out["events"]["m.space.child"], 50);
        assert!(out["events"].get("m.space.parent").is_none());
        let child = derive_power_levels(&cur, &st, false);
        assert_eq!(child["events"]["m.space.parent"], 50);
        assert!(child["events"].get("m.space.child").is_none());
        // A non-default users_default is honoured.
        let cur3 = json!({"users_default": 10, "users": {"@zane:example.com": 10}});
        let out3 = derive_power_levels(&cur3, &st, true);
        assert!(out3["users"].get("@zane:example.com").is_none());
        assert_eq!(out3["users"]["@blake:example.com"], 100);
    }

    #[test]
    fn derivation_is_idempotent_and_deterministic() {
        let st = template_state();
        let cur = stock_pl();
        let once = derive_power_levels(&cur, &st, true);
        let twice = derive_power_levels(&once, &st, true);
        assert_eq!(once, twice);
        assert_eq!(once.to_string(), twice.to_string());
        assert_eq!(
            derive_power_levels(&cur, &st, false).to_string(),
            derive_power_levels(&cur, &st, false).to_string()
        );
        // From nothing at all it still produces a full, self-consistent event.
        let empty = derive_power_levels(&json!({}), &st, true);
        assert_eq!(empty["kick"], 50);
        assert_eq!(empty["users"]["@blake:example.com"], 100);
        assert_eq!(derive_power_levels(&empty, &st, true), empty);
        // Non-object input is treated as empty rather than panicking.
        assert_eq!(derive_power_levels(&Value::Null, &st, true), empty);
    }

    #[test]
    fn perms_from_power_levels_maps_every_key() {
        let pl = stock_pl();
        let admin = perms_from_power_levels(100, &pl);
        assert_eq!(admin, all_permission_keys());
        let modp = perms_from_power_levels(50, &pl);
        for k in [
            "kick",
            "ban",
            "invite",
            "manage_messages",
            "pin_messages",
            "mention_everyone",
            "moderate_calls",
            "manage_space",
            "manage_rooms",
            "manage_soundboard",
            "record_calls",
            "manage_games",
            "share_screen",
            "use_soundboard",
            "manage_calendar",
            "manage_watchparty",
        ] {
            assert!(modp.contains(k), "mod lacks {k}");
        }
        assert!(!modp.contains("manage_roles"));
        let member = perms_from_power_levels(0, &pl);
        assert!(member.contains("invite"));
        assert!(member.contains("share_screen"));
        assert!(!member.contains("kick"));
        assert!(!member.contains("moderate_calls"));
        // An explicit events entry beats state_default; a missing PL falls back to defaults.
        let pl2 = json!({"events": {"dev.tinhat.call.moderation": 0, "m.room.name": 100}});
        let p2 = perms_from_power_levels(0, &pl2);
        assert!(p2.contains("moderate_calls"));
        assert!(!p2.contains("manage_space"));
        assert!(perms_from_power_levels(50, &json!({})).contains("kick"));
        assert!(!perms_from_power_levels(49, &json!({})).contains("kick"));
        assert!(!member.contains("manage_game_servers"));
        assert!(!perms_from_power_levels(99, &json!({})).contains("manage_game_servers"));
        assert!(perms_from_power_levels(100, &json!({})).contains("manage_game_servers"), "the owner rank");
        // widgets W1: without roles the note follows the room's power levels (state_default 50 unless provisioned to 0)
        assert!(!perms_from_power_levels(0, &json!({})).contains("edit_space_note"));
        assert!(perms_from_power_levels(0, &json!({"events": {"dev.tinhat.note": 0}})).contains("edit_space_note"));
        let j: Value = serde_json::from_str(&perms_json(&member)).unwrap();
        assert_eq!(j.as_object().unwrap().len(), 19);
        assert_eq!(j["invite"], true);
        assert_eq!(j["ban"], false);
    }

    #[test]
    fn default_template_shape() {
        let t = default_template();
        assert_eq!(t.len(), 3);
        assert_eq!(t[0].id, "admin");
        assert_eq!(t[0].rank, 100);
        assert_eq!(t[0].permissions.len(), 19);
        assert!(t[0].permissions.contains("manage_game_servers"), "the Admin preset holds every permission");
        assert!(!t[1].permissions.contains("manage_game_servers"), "Moderator does not");
        assert_eq!(t[1].id, "moderator");
        assert_eq!(t[1].rank, 50);
        assert_eq!(t[1].permissions.len(), 17);
        assert_eq!(t[2].id, EVERYONE);
        assert_eq!(t[2].rank, 0);
        assert_eq!(
            t[2].permissions,
            perms(&["invite", "share_screen", "use_soundboard", "record_calls", "edit_space_note"])
        );
        for r in &t {
            for p in &r.permissions {
                assert!(is_known_permission(p), "{p}");
            }
        }
    }

    #[test]
    fn new_role_ids_look_right() {
        let a = new_role_id();
        let b = new_role_id();
        assert_eq!(a.len(), 10);
        assert!(a.starts_with("r-"));
        assert!(a[2..].chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn members_at_or_above() {
        let st = template_state();
        assert_eq!(st.members_at_or_above(100), vec!["@blake:example.com".to_string()]);
        assert_eq!(st.members_at_or_above(50).len(), 2);
        assert_eq!(st.members_at_or_above(0).len(), 3);
        assert_eq!(st.without_role("admin").members_at_or_above(100).len(), 0);
    }
}
