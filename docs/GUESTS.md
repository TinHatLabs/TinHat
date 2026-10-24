# The guest room — the desktop half

Contract: the server repo's `docs/GUESTS.md` (binding: the words, who may do what, the room state `dev.tinhat.guests` /
`dev.tinhat.guests.waiting`, the API, the apps' places). What the desktop had to do: the web repo's
`docs/DESKTOP-PENDING-guests.md` §2–§3 and §6 (§1, the offer, is `server_offer.rs`: `ServerFeature::Guests`,
`PANE_GATES`, `RETIRED_ACTIONS`, `is_guest_account`). Web twin: `src/features/guests/` (model.ts + model.test.ts,
store.ts, watch.ts, GuestsBar.tsx, GuestsPane.tsx, index.tsx). Desktop and web are one product: every word below is the
web's, byte for byte.

The desktop is a self-hosted build only (docs/SELFHOST.md): the guest room is there whenever the server's offer doesn't
say `guests: false`.

## Files

| file | what |
|---|---|
| `crates/app/src/guests.rs` | the shared rules — model.ts line for line: `GUESTS_TYPE`, `WAITING_TYPE`, `MAX_WAITING` 20, `NAME_MAX` 32, `WAIT_MS` 600 000, `MANAGE_LEVEL` 50, `SEEN_MAX` 100; `WORDS`, `EXPLAIN`, `waiting_text`, `notice_text`, the bar / pane labels (`BarButton::label`, `PaneButton::label`); `parse_guest_state`, `parse_waiting`, `clean_name`, `parse_doc`, `local_role`, `merge_role`, `bar_plan`, `waiting_rows`, `pane_plan`, `is_guests_room`, `arrivals`, `departures`, `account_name`. JS's value rules are kept exactly (JS `trim` / `\s`, `Number.isInteger`, code points). |
| `crates/app/src/guests_ctl.rs` | `GuestsController` (QML `guestsCtl`): the API (store.ts), following THE Guests room through sync, noticing (watch.ts), and the one JSON QML draws (`view_json`). |
| `crates/app/src/notify.rs` | `notify_action_id` (the OS notification's id) + `close_notification` (`CloseNotification`) — Linux; on Windows a WinRT toast can't be withdrawn (it expires by itself) and the id is None. |
| `crates/app/src/bridge.rs` | one line in the sync loop (`guests_ctl::on_sync`); a guest account skips the first-device set-up (`set_up_first_device` returns at once, `first_device_setup` passes `guest`). |
| `crates/app/qml/AppCore.qml` | `GuestsController { id: guestsCtl }`; `mGuests` (the JSON, re-parsed when `rev` moves), `mGuestsHere` (the open room is THE Guests room); `start()` at sign-in / `reset()` at sign-out (before the logged-out guard of the mirror loop); `offerChanged()` when the server's offer changes. |
| `crates/app/qml/Main.qml` | `guestsCtl` / `mGuests` / `mGuestsHere`; `Connections` on the controller: `toast` → `settingsToast` (where the person is looking), `copyReady` → clipboard + toast, `openReady` → Settings closes, the room opens; `onToastAction` `guest-waiting:<room>\t<text>` → the in-app toast with Open; §6 the verify banner / sign-in prompt never for a guest account. |
| `crates/app/qml/HeaderIsland.qml` | the bar under THE Guests room's header (first under the header, before the call and Media Party banners). |
| `crates/app/qml/SettingsWindow.qml` | Settings → Guests (rail: Account, after Invites). |

## Where (the contract's "The apps")

**Settings → Guests** — rail entry `guests`, label "Guests", icon `system-users-symbolic`, description "Let people in for a
while without an account"; shown while `app.paneOffered("guests")` (PANE_GATES) and the account isn't a guest
(`app.mGuestAccount`). Body = `pane_plan`: the section "Guest room" with `EXPLAIN` as its hint; before the first answer
"Checking…" (or the failure, in `settings.danger.fg`); a row with the state text and the plan's buttons (Turn on the guest
room highlighted, Turn off / New link flat; one or two beside the text, a manager's four under it, wrapping; all but Copy /
Open wait while a turn on / off / new link is out); "Guest link" as selectable monospace text while `showLink`. The doc is
read when the pane shows; on/off follows the room state (sync) when it is newer than the doc. "Open the Guests room" joins
the room first when this user isn't in it (its join rule is public), then opens it and closes Settings.

**The bar** — only in THE Guests room (`is_guests_room`: the doc's `room`; before the doc, a room with `dev.tinhat.guests`
whose canonical alias is `#guests:<my server>`). The account-ask card's look (`Th.accountAsk{Bg,Border,Fg,Icon}`, radius 12):
icon `system-users-symbolic`, `bar_plan`'s line + buttons (Turn on highlighted, the rest flat; all but Copy wait while a
manage request is out), then per `waiting_rows` entry a hairline and "**Ana** is waiting" (StyledText; the name escaped in
Rust) + [Let in] (highlighted) [Turn away] (flat), both disabled while that id's request is out. Guests, people who may not
see the state and servers whose offer says no get no bar (`bar: null`). The doc is read when the bar shows and whenever the
room's `dev.tinhat.guests` changes.

**The API** (`proxy::url` = the base from the server's well-known; the session's Matrix access token as bearer):
`GET /guests` (12 s); `POST /guests/on`, `/guests/off`, `/guests/link` (body `{}`, 20 s, → the doc);
`POST /guests/admit {id}`, `/guests/turn-away {id}` (→ `{ok}`). Errors: 404 → "This server doesn't have the guest room
yet."; no answer → "Couldn't reach the server — try again in a moment."; else the answer's `error`, or "The server said
<status>"; a 200 that isn't a doc → "The server sent something unexpected.". Toasts (store.ts): "The guest room is on — copy
the guest link to send it to someone" / "The guest room is off — whoever waited was turned away and the guests are gone" /
"New guest link — the old one stopped working" (5 s); "Ana is coming in" / "Ana was turned away" (4 s); "Guest link copied
— whoever opens it waits at the door until someone lets them in" (5 s); failures "Couldn't turn the guest room on: <error>"
/ "… off: …" / "Couldn't make a new link: …" / "Couldn't let Ana in: …" / "Couldn't turn Ana away: …" (6 s); no link to copy:
the read's error, else "The guest room is off — there's no guest link right now." when off, else "There's no guest link for
you to copy."; a failed join: "Couldn't open the Guests room: <the Matrix error>". **Copy guest link** reads the doc first
unless it is under 30 s old (a new link writes no room state). One read at a time; a session's answers never reach the
next session (the generation guard).

**Noticing** (watch.ts) — `on_sync`: a room whose `dev.tinhat.guests`, `dev.tinhat.guests.waiting` or canonical alias changed,
THE Guests room's power levels, or my own membership anywhere (joined it just now) is looked at again from the store (the
first pass of a session looks at every room carrying the state, and the doc's room). For THE Guests room: `departures`
close their notices; `eligible` = the server offers guests ∧ on (sync, else the doc) ∧ joined ∧ not a guest account;
`arrivals` against the device pref `ui-guests-seen` (a JSON array of ids, the web's `guests.seen`; kept across sign-outs as
the web keeps it); per arrival: the notify chime always (`sound:notify`), and — unless the Guests room is the open room in a
focused window (`notify::is_open_room`) — the in-app toast "Ana is waiting to come in as a guest" with Open (8 s) and, with
Desktop notifications on, the OS notification of the same text (click → the room, `open:<room>`). The OS notification closes
when that guest leaves the list (Linux). The server's offer changing re-runs the look at every room.

**Guest accounts on the desktop** (§6; none expected — guests come in through the browser): no first-device encryption
set-up, no "Verify this device" banner, no verify dialog at sign-in, no Settings → Guests, no bar, no notices.

## QML entry points

`guestsCtl.start()` / `reset()` / `refresh()` / `offerChanged()` / `turnOn()` / `turnOff()` / `newLink()` / `copyLink()` /
`letIn(id)` / `turnAway(id)` / `openRoom()` / `isGuestAccount(mxid)`; signals `toast(text, timeoutMs)`,
`copyReady(link, text)`, `openReady(roomId)`; `rev` + `stateJson` = `{offered, guest, room, on, bar: {text, buttons:
[{id, label, highlighted, flat, enabled}]} | null, rows: [{id, name, html, enabled}], pane: {state, text, buttons,
showLink} | null, link, note, noteError, busy, words: {pane, section, explain, letIn, turnAway, link}}`.

## Tests

- `guests::tests` — model.test.ts's cases (15): the words and event types, `parse_guest_state`, `parse_waiting`,
  `clean_name`, `parse_doc`, `is_guest_account`, `local_role`, `merge_role`, `bar_plan`, `waiting_rows`, `pane_plan`,
  `is_guests_room`, `arrivals`, `departures`, `account_name`. Not here: the web's "a guest account gets no Report a
  problem" (server_offer.rs's tests cover `GUEST_WITHHELD` through the real engine) and `handoffJoinsCall` (web only).
- `guests_ctl::tests` — the API side's words byte for byte; what QML is handed (bar, rows, pane, the sync-vs-doc rules, a
  server admin outside the room, a guest account, the offer off, "Checking…" / errors); the noticing step (once per id,
  departures close, eligibility).

## Not on the desktop (web only)

The hand-off (`#/guest-session`, `handoffJoinsCall`), a guest account's footer name (`account_name` is ported for the
shared tests; the desktop signs in with a password), the Settings search entries for the pane (`guests.state` /
`guests.link` in the shared `settings-index.json` — a contract file both repos copy byte for byte; to add together).
