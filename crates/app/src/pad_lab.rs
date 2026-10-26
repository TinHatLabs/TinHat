//! pad_lab.rs — TINHAT_PAD_TEST: the headless Pad gate on a LAB server (docs/PAD.md; DESIGN §7). It refuses
//! anything but a local address or a `*.test` name, and wants two throwaway accounts (TINHAT_USER/PASS = "me",
//! TINHAT_USER2/PASS2 = a friend). In one process, with a store each (a temp dir; the app's own data is never touched):
//!   R  = my receiver (cross-signing set up here + a Security Key), "in a call" (its snapshot; no LiveKit here)
//!   C  = my controller, verified with that Security Key
//!   X  = my third device, signed in but never verified
//!   F  = the friend's device
//! Each runs the SHIPPED node (pad_net.rs → pad_engine.rs → padrules.rs) fed by its own sync loop, exactly as the
//! app's sync loop feeds it; R's executor is a spy (it records the commands and confirms them). Checked: discovery
//! (C sees R ready; X sees "unverified"); not-in-call; open → the call; a sound tap runs once and is acked; a
//! replayed seq is dropped; a stale command is refused; mute; volumes (a participant ok, a stranger `not-here`); X's
//! crafted commands REJECTED (peer-untrusted) with nothing back; F's REJECTED (other-user); a plaintext command
//! REJECTED (not-olm); leaving the call pushes `ended`; × closes the session and refuses the device until the call
//! changes; Pad off ends it with `off`. Real calls (LiveKit, sounds heard by a peer) are phase 3.
//! Output: `[padlab] ok|FAIL …` lines and `[padlab] VERDICT: PASS|FAIL`; every device signs itself out at the end.
use crate::pad_net::{now_ms, PadEvent, PadNode};
use crate::padrules::PAD_CMD;
use matrix_sdk::ruma::OwnedDeviceId;
use matrix_sdk::Client;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const FAV: &str = "https://sounds.example.org/bruh.mp3";

fn lab_server(url: &str) -> bool {
    let host = url::Url::parse(url).ok().and_then(|u| u.host_str().map(|h| h.trim_end_matches('.').to_ascii_lowercase()));
    matches!(host.as_deref(), Some(h) if h == "127.0.0.1" || h == "localhost" || h == "::1" || h == "[::1]" || h.ends_with(".test"))
}

struct Dev {
    tag: &'static str,
    client: Client,
    node: Arc<PadNode>,
    events: Arc<Mutex<Vec<PadEvent>>>,
}

impl Dev {
    fn device(&self) -> String {
        self.client.device_id().map(|d| d.to_string()).unwrap_or_default()
    }
    fn user(&self) -> String {
        self.client.user_id().map(|u| u.to_string()).unwrap_or_default()
    }
    fn logs(&self) -> Vec<String> {
        self.events.lock().unwrap().iter().filter_map(|e| if let PadEvent::Log(l) = e { Some(l.clone()) } else { None }).collect()
    }
    fn count(&self, f: impl Fn(&PadEvent) -> bool) -> usize {
        self.events.lock().unwrap().iter().filter(|e| f(e)).count()
    }
    fn remote(&self) -> Value {
        self.node.remote_view()
    }
    fn mark(&self) -> usize {
        self.events.lock().unwrap().len()
    }
    /// The receiver's answer to command `seq`, among the events since `mark` (a seq is reused by a new session).
    fn ack_since(&self, mark: usize, seq: i64) -> Option<(bool, String)> {
        self.events.lock().unwrap().iter().skip(mark).find_map(|e| match e {
            PadEvent::Ack { seq: s, ok, why, .. } if *s == seq => Some((*ok, why.clone())),
            _ => None,
        })
    }
}

async fn wait_until(ms: u64, mut f: impl FnMut() -> bool) -> bool {
    let t = std::time::Instant::now();
    loop {
        if f() {
            return true;
        }
        if t.elapsed() > Duration::from_millis(ms) {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn lab_client(server: &str, dir: &std::path::Path, user: &str, pass: &str, name: &str) -> Result<Client, String> {
    use matrix_sdk::encryption::{BackupDownloadStrategy, EncryptionSettings};
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let c = Client::builder()
        .homeserver_url(server)
        .sqlite_store(dir, None)
        .with_encryption_settings(EncryptionSettings { auto_enable_cross_signing: false, backup_download_strategy: BackupDownloadStrategy::Manual, auto_enable_backups: false })
        .build()
        .await
        .map_err(|e| format!("{name}: client: {e}"))?;
    c.matrix_auth().login_username(user, pass).initial_device_display_name(name).await.map_err(|e| format!("{name}: login: {e}"))?;
    Ok(c)
}

/// A device: its node (pad on) + a sink that records events; R's sink also runs the executor spy.
fn dev(tag: &'static str, client: Client, spy: Option<tokio::sync::mpsc::UnboundedSender<(i64, Value)>>) -> Dev {
    let events: Arc<Mutex<Vec<PadEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let ev2 = events.clone();
    let node = PadNode::new(
        client.clone(),
        true,
        Arc::new(move |e: PadEvent| {
            if let (PadEvent::Exec { token, cmds }, Some(tx)) = (&e, spy.as_ref()) {
                let _ = tx.send((*token, cmds.clone()));
            }
            ev2.lock().unwrap().push(e);
        }),
    );
    Dev { tag, client, node, events }
}

/// The sync loop the app runs (bridge.rs): every to-device event into the node.
fn sync_loop(d: &Dev, stop: Arc<AtomicBool>) {
    let (c, n, tag) = (d.client.clone(), d.node.clone(), d.tag);
    crate::bridge::runtime().spawn(async move {
        let settings = crate::bridge::sync_settings().timeout(Duration::from_millis(1_000));
        while !stop.load(Ordering::SeqCst) {
            match c.sync_once(settings.clone()).await {
                Ok(r) => {
                    for pe in &r.to_device {
                        n.on_to_device(pe).await;
                    }
                }
                Err(e) => {
                    eprintln!("[padlab] {tag}: sync: {e}");
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }
        }
    });
}

fn snapshot(room: &str, gen: &str, friend: &str) -> Value {
    json!({
        "call": {"room": room, "gen": gen, "since": now_ms()},
        "me": {"micMuted": false, "ptt": false, "serverMuted": false, "deaf": false, "brb": false, "talk": false, "sound": "",
               "gate": {"allowed": true, "hint": ""}},
        "people": [{"u": friend, "talk": false, "micOff": false, "away": false}],
        "recent": [], "favourites": [FAV],
    })
}

pub async fn headless_pad_test() -> bool {
    let server = match crate::bridge::env_homeserver() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[padlab] FAIL {e}");
            return false;
        }
    };
    if !lab_server(&server) {
        eprintln!("[padlab] FAIL refusing {server}: this gate signs throwaway devices in and out — a lab server only (local or *.test)");
        return false;
    }
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    let (user, pass, user2, pass2) = (env("TINHAT_USER"), env("TINHAT_PASS"), env("TINHAT_USER2"), env("TINHAT_PASS2"));
    if user.is_empty() || pass.is_empty() || user2.is_empty() || pass2.is_empty() {
        eprintln!("[padlab] FAIL set TINHAT_USER/PASS and TINHAT_USER2/PASS2 (two throwaway lab accounts)");
        return false;
    }
    let base = std::env::temp_dir().join(format!("tinhat-padlab-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let ok = run(&server, &base, &user, &pass, &user2, &pass2).await;
    let _ = std::fs::remove_dir_all(&base);
    eprintln!("[padlab] VERDICT: {}", if ok { "PASS" } else { "FAIL" });
    ok
}

async fn run(server: &str, base: &std::path::Path, user: &str, pass: &str, user2: &str, pass2: &str) -> bool {
    let mut fails = 0;
    let mut check = |label: &str, ok: bool| {
        eprintln!("[padlab] {} {label}", if ok { "ok  " } else { "FAIL" });
        if !ok {
            fails += 1;
        }
    };
    let stop = Arc::new(AtomicBool::new(false));

    // ---- the devices ----
    let login = |dir: &'static str, u: String, p: String, name: &'static str| {
        let (server, base) = (server.to_string(), base.join(dir));
        async move { lab_client(&server, &base, &u, &p, name).await }
    };
    let r_c = match login("r", user.into(), pass.into(), "pad-lab R").await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[padlab] FAIL {e}");
            return false;
        }
    };
    if let Err(e) = r_c.sync_once(crate::bridge::sync_settings()).await {
        eprintln!("[padlab] FAIL R: first sync: {e}");
        return false;
    }
    if let Err(e) = r_c.encryption().bootstrap_cross_signing(None).await {
        eprintln!("[padlab] FAIL R: cross-signing (use a fresh account): {e}");
        return false;
    }
    let key = match r_c.encryption().recovery().enable().await {
        Ok(k) => k,
        Err(e) => {
            eprintln!("[padlab] FAIL R: Security Key: {e}");
            return false;
        }
    };
    let (spy_tx, mut spy_rx) = tokio::sync::mpsc::unbounded_channel::<(i64, Value)>();
    let r = dev("R", r_c, Some(spy_tx));
    sync_loop(&r, stop.clone());

    let c_c = match login("c", user.into(), pass.into(), "pad-lab C").await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[padlab] FAIL {e}");
            return false;
        }
    };
    let _ = c_c.sync_once(crate::bridge::sync_settings()).await;
    if let Err(e) = c_c.encryption().recovery().recover(&key).await {
        eprintln!("[padlab] FAIL C: verify with the Security Key: {e}");
        return false;
    }
    let c = dev("C", c_c, None);
    sync_loop(&c, stop.clone());
    let x = match login("x", user.into(), pass.into(), "pad-lab X (unverified)").await {
        Ok(cl) => dev("X", cl, None),
        Err(e) => {
            eprintln!("[padlab] FAIL {e}");
            return false;
        }
    };
    sync_loop(&x, stop.clone());
    let f = match login("f", user2.into(), pass2.into(), "pad-lab F").await {
        Ok(cl) => dev("F", cl, None),
        Err(e) => {
            eprintln!("[padlab] FAIL {e}");
            return false;
        }
    };
    sync_loop(&f, stop.clone());
    let (me, friend, rdev, cdev, xdev) = (r.user(), f.user(), r.device(), c.device(), x.device());

    // the executor spy: records what R's UI would run, then confirms it (as PadHost does after running it)
    let ran: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let (ran, rn) = (ran.clone(), r.node.clone());
        crate::bridge::runtime().spawn(async move {
            while let Some((token, cmds)) = spy_rx.recv().await {
                // as PadHost.run: claim against the call this device is in (here: its snapshot), run, confirm
                if !rn.claim(token, rn.snapshot_call()) {
                    continue;
                }
                for c in cmds.as_array().cloned().unwrap_or_default() {
                    ran.lock().unwrap().push(c);
                }
                rn.executed(token);
            }
        });
    }

    // ---- trust as each side sees it ----
    check("C (verified with the Security Key) trusts R", trusted(&c.client, &me, &rdev, 20_000).await);
    let r_trusts_c = trusted(&r.client, &me, &cdev, 20_000).await;
    check("R trusts C", r_trusts_c);
    check("R does not trust X (never verified)", !trusted(&r.client, &me, &xdev, 2_000).await);

    // ---- the call room + R's call membership (the pad1 cap; no LiveKit here) ----
    let room_id = {
        use matrix_sdk::ruma::api::client::room::create_room::v3::Request as Create;
        let mut req = Create::new();
        req.name = Some("Pad lab".into());
        req.invite = matrix_sdk::ruma::UserId::parse(friend.as_str()).map(|u| vec![u]).unwrap_or_default();
        match r.client.create_room(req).await {
            Ok(room) => room.room_id().to_owned(),
            Err(e) => {
                eprintln!("[padlab] FAIL create room: {e}");
                return false;
            }
        }
    };
    let room = room_id.to_string();
    let joined = wait_until(15_000, || f.client.get_room(&room_id).is_some()).await;
    if joined {
        if let Some(fr) = f.client.get_room(&room_id) {
            if let Err(e) = fr.join().await {
                eprintln!("[padlab] F: join: {e}");
            }
        }
    }
    check("F joined the call room", joined);
    let rr = match r.client.get_room(&room_id) {
        Some(x) => x,
        None => {
            eprintln!("[padlab] FAIL R: no room");
            return false;
        }
    };
    if let Err(e) = crate::rtc::join_rtc_session(&rr, "https://lab.test/livekit/jwt").await {
        eprintln!("[padlab] FAIL R: call membership: {e}");
        return false;
    }

    // ---- discovery (padTargets on each device's own room state) ----
    let targets_of = |d: &Dev| {
        let (n, cl, rid) = (d.node.clone(), d.client.clone(), room_id.clone());
        async move {
            let Some(room) = cl.get_room(&rid) else { return Value::Null };
            n.targets(&[room]).await
        }
    };
    let mut seen = Value::Null;
    for _ in 0..60 {
        seen = targets_of(&c).await;
        if seen["rooms"][&room].is_array() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let ct = seen["rooms"][&room].as_array().cloned().unwrap_or_default();
    check("C discovers R in the call, ready (pad1, trusted)", ct.len() == 1 && ct[0]["device"] == rdev.as_str() && ct[0]["ready"] == true);
    let mut xs = Value::Null;
    for _ in 0..60 {
        xs = targets_of(&x).await;
        if xs["rooms"][&room].is_array() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let xt = xs["rooms"][&room].as_array().cloned().unwrap_or_default();
    check("X (unverified) sees R greyed: \"unverified\"", xt.len() == 1 && xt[0]["why"] == "unverified" && xt[0]["ready"] == false);
    let ft = targets_of(&f).await;
    check("F (another account) has no Pad target", ft["rooms"][&room].is_null());

    // ---- R not in a call ----
    r.node.publish_state(&json!({"call": null}));
    c.node.open_pad(&room, &rdev);
    let ended_nic = wait_until(10_000, || c.count(|e| matches!(e, PadEvent::Ended(w) if w == "not-in-call")) > 0).await;
    check("open while R isn't in a call → not-in-call", ended_nic && c.remote()["state"]["call"].is_null());

    // ---- R in the call ----
    r.node.publish_state(&snapshot(&room, "lab1", &friend));
    let t0 = now_ms();
    c.node.open_pad(&room, &rdev);
    let opened = wait_until(10_000, || c.remote()["opened"] == true).await;
    check("open → the call (room, gen)", opened && c.remote()["state"]["call"]["room"] == room.as_str() && c.remote()["gen"] == "lab1");
    eprintln!("[padlab] open answered in {} ms", now_ms() - t0);
    check("R shows the chip for C", wait_until(3_000, || r.count(|e| matches!(e, PadEvent::Chips(d) if d.contains(&cdev))) > 0).await);

    // a sound tap: runs once, acked ok
    let mut lat: Vec<i64> = Vec::new();
    let (t1, m) = (now_ms(), c.mark());
    let seq = c.node.send(&json!({"op": "sound.play", "url": FAV}));
    let got = wait_until(8_000, || c.ack_since(m, seq).is_some()).await;
    lat.push(now_ms() - t1);
    let played = ran.lock().unwrap().iter().filter(|x| x["do"] == "startSound" && x["url"] == FAV).count();
    check("a sound tap runs startSound once and is acked ok", got && played == 1 && c.ack_since(m, seq) == Some((true, String::new())));

    // replay: the same message (same seq) again → dropped, nothing runs twice
    let content = json!({"v": 1, "sid": c.remote()["sid"], "seq": seq, "ts": now_ms(), "room": room, "gen": "lab1", "op": "sound.play", "url": FAV});
    let _ = PadNode::send_raw(&c.client, &rdev, PAD_CMD, &content).await;
    let rejected = wait_until(5_000, || r.logs().iter().any(|l| l.contains("REJECTED (old-seq)"))).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let played = ran.lock().unwrap().iter().filter(|x| x["do"] == "startSound").count();
    check("a replayed seq is dropped (old-seq) and runs nothing", rejected && played == 1);

    // stale: a command sent 20 s ago (by its own clock) → refused
    let stale_seq = 900;
    let content = json!({"v": 1, "sid": c.remote()["sid"], "seq": stale_seq, "ts": now_ms() - 20_000, "room": room, "gen": "lab1", "op": "mic", "muted": true});
    let _ = PadNode::send_raw(&c.client, &rdev, PAD_CMD, &content).await;
    let stale = wait_until(6_000, || c.remote()["state"]["re"] == stale_seq && c.remote()["state"]["why"] == "stale").await;
    check("a command held 20 s → stale", stale && !ran.lock().unwrap().iter().any(|x| x["do"] == "muteMic"));
    // the crafted seq 900 used up the seqs below it: a new session (a new sid starts at 1; it replaces the old one)
    c.node.open_pad(&room, &rdev);
    check("a second open from C replaces its first session", wait_until(10_000, || c.remote()["opened"] == true).await);

    // mute
    let (t2, m) = (now_ms(), c.mark());
    let seq = c.node.send(&json!({"op": "mic", "muted": true}));
    let got = wait_until(8_000, || c.ack_since(m, seq).is_some()).await;
    lat.push(now_ms() - t2);
    check("mute → muteMic, acked ok", got && ran.lock().unwrap().iter().any(|x| x["do"] == "muteMic") && c.ack_since(m, seq) == Some((true, String::new())));

    // volumes: a participant ok; a stranger not-here
    let (t3, m) = (now_ms(), c.mark());
    let seq = c.node.send(&json!({"op": "vol", "user": friend, "cat": "mic", "level": 0.3}));
    let got = wait_until(8_000, || c.ack_since(m, seq).is_some()).await;
    lat.push(now_ms() - t3);
    check(
        "their mic to 30 % → setVolume, acked ok",
        got && ran.lock().unwrap().iter().any(|x| x["do"] == "setVolume" && x["user"] == friend.as_str() && x["level"].as_f64() == Some(0.3)),
    );
    let m = c.mark();
    let seq = c.node.send(&json!({"op": "vol", "user": "@stranger:lab.test", "cat": "mic", "level": 0.3}));
    let got = wait_until(8_000, || c.ack_since(m, seq).is_some()).await;
    check("a stranger's volume → not-here", got && c.ack_since(m, seq) == Some((false, "not-here".into())));

    // ---- X: my own but unverified device → REJECTED, nothing back ----
    let before_x = r.logs().iter().filter(|l| l.contains("REJECTED (peer-untrusted)")).count();
    let open_x = json!({"v": 1, "sid": "WFdYV1hXWFdYV1hX", "seq": 1, "ts": now_ms(), "room": room, "op": "open"});
    let sent_x = PadNode::send_raw(&x.client, &rdev, PAD_CMD, &open_x).await;
    let cmd_x = json!({"v": 1, "sid": "WFdYV1hXWFdYV1hX", "seq": 2, "ts": now_ms(), "room": room, "gen": "lab1", "op": "sound.play", "url": FAV});
    let _ = PadNode::send_raw(&x.client, &rdev, PAD_CMD, &cmd_x).await;
    let rej = wait_until(8_000, || r.logs().iter().filter(|l| l.contains("REJECTED (peer-untrusted)")).count() >= before_x + 2).await;
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    check(
        "X's open + command: REJECTED (peer-untrusted), no state back, nothing runs",
        sent_x.is_ok() && rej && x.count(|e| matches!(e, PadEvent::Log(l) if l.starts_with("state dropped"))) == 0 && ran.lock().unwrap().iter().filter(|x| x["do"] == "startSound").count() == 1,
    );

    // ---- F: another account → REJECTED ----
    let have_r = {
        let mut ok = false;
        for _ in 0..40 {
            let u = matrix_sdk::ruma::UserId::parse(me.as_str()).ok();
            if let Some(u) = u {
                if f.client.encryption().get_device(&u, &OwnedDeviceId::from(rdev.as_str())).await.ok().flatten().is_some() {
                    ok = true;
                    break;
                }
                let _ = f.client.encryption().request_user_identity(&u).await;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        ok
    };
    let cmd_f = json!({"v": 1, "sid": "RkZGRkZGRkZGRkZG", "seq": 1, "ts": now_ms(), "room": room, "op": "open"});
    let sent_f = PadNode::send_raw_to(&f.client, &me, &rdev, PAD_CMD, &cmd_f).await;
    let rej_f = wait_until(8_000, || r.logs().iter().any(|l| l.contains("REJECTED (other-user)"))).await;
    check("F's Olm command: REJECTED (other-user)", have_r && sent_f.is_ok() && rej_f);

    // ---- plaintext → REJECTED (not-olm) ----
    let plain = json!({"v": 1, "sid": c.remote()["sid"], "seq": 5_000, "ts": now_ms(), "room": room, "gen": "lab1", "op": "sound.play", "url": FAV});
    let sent_p = send_plaintext(&c.client, &me, &rdev, &plain).await;
    let rej_p = wait_until(8_000, || r.logs().iter().any(|l| l.contains("REJECTED (not-olm)"))).await;
    check("a plaintext command: REJECTED (not-olm), nothing runs", sent_p.is_ok() && rej_p && ran.lock().unwrap().iter().filter(|x| x["do"] == "startSound").count() == 1);

    // ---- R leaves the call → ended; back in a new call ----
    r.node.publish_state(&json!({"call": null}));
    let ended = wait_until(5_000, || c.remote()["state"]["call"].is_null() && c.remote()["state"]["why"] == "ended").await;
    check("R leaves the call → a final state \"ended\"", ended);
    r.node.publish_state(&snapshot(&room, "lab2", &friend));
    let back = wait_until(5_000, || c.remote()["gen"] == "lab2").await;
    check("R in a new call → the session follows (new gen)", back);

    // ---- × on R's chip → closed, and refused until the call changes ----
    r.node.refuse(&cdev);
    let closed = wait_until(5_000, || c.count(|e| matches!(e, PadEvent::Ended(w) if w == "closed")) > 0).await;
    check("× on the chip → C's session ends \"closed\"", closed && r.node.remote_view().is_null());
    c.node.open_pad(&room, &rdev);
    let refused = wait_until(8_000, || c.count(|e| matches!(e, PadEvent::Ended(w) if w == "closed")) >= 2).await;
    check("… and C is refused (closed) for the rest of that call", refused);
    r.node.publish_state(&snapshot(&room, "lab3", &friend));
    c.node.open_pad(&room, &rdev);
    let reopened = wait_until(8_000, || c.remote()["opened"] == true).await;
    check("a new call: C may open again", reopened);

    // ---- Pad off on R → off ----
    r.node.set_pad_on(false);
    let off = wait_until(5_000, || c.count(|e| matches!(e, PadEvent::Ended(w) if w == "off")) > 0).await;
    check("Pad off on R → the session ends \"off\"", off);

    lat.sort();
    if !lat.is_empty() {
        eprintln!("[padlab] command → ack on the lab: {:?} ms (p50 {} ms)", lat, lat[lat.len() / 2]);
    }

    // ---- clean up: every device signs out ----
    stop.store(true, Ordering::SeqCst);
    c.node.close_pad();
    tokio::time::sleep(Duration::from_millis(1_200)).await;
    for d in [&r, &c, &x, &f] {
        if let Err(e) = d.client.matrix_auth().logout().await {
            eprintln!("[padlab] {}: sign out: {e}", d.tag);
        }
    }
    fails == 0
}

async fn trusted(c: &Client, me: &str, device: &str, ms: u64) -> bool {
    let t = std::time::Instant::now();
    let Ok(u) = matrix_sdk::ruma::UserId::parse(me) else { return false };
    loop {
        if let Ok(Some(d)) = c.encryption().get_device(&u, &OwnedDeviceId::from(device)).await {
            if d.is_verified_with_cross_signing() {
                return true;
            }
        }
        if t.elapsed() > Duration::from_millis(ms) {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// An UNENCRYPTED `dev.tinhat.pad.cmd` (what an attacker with server access could inject).
async fn send_plaintext(c: &Client, user: &str, device: &str, content: &Value) -> Result<(), String> {
    use matrix_sdk::ruma::api::client::to_device::send_event_to_device::v3::Request;
    use matrix_sdk::ruma::events::{AnyToDeviceEventContent, ToDeviceEventType};
    use matrix_sdk::ruma::serde::Raw;
    use matrix_sdk::ruma::to_device::DeviceIdOrAllDevices;
    use std::collections::BTreeMap;
    let uid = matrix_sdk::ruma::UserId::parse(user).map_err(|e| e.to_string())?;
    let raw: Raw<AnyToDeviceEventContent> = Raw::from_json(serde_json::value::to_raw_value(content).map_err(|e| e.to_string())?);
    let mut per = BTreeMap::new();
    per.insert(DeviceIdOrAllDevices::DeviceId(OwnedDeviceId::from(device)), raw);
    let mut msgs = BTreeMap::new();
    msgs.insert(uid, per);
    let req = Request::new_raw(ToDeviceEventType::from(PAD_CMD), matrix_sdk::ruma::TransactionId::new(), msgs);
    c.send(req).await.map(|_| ()).map_err(|e| e.to_string())
}

/// TINHAT_PAD_TEST=seed — the screenshots' lab state (pad-shots.sh): account TINHAT_USER signs in a device "Desktop PC"
/// (cross-signing set up, so it is the account's verified device), makes a room "Game night" with three room sounds and
/// three favourites, and puts that device in the room's call (a real membership with the `pad1` cap; no LiveKit).
/// It stays signed in (the GUI's other device then really sees "You're in Game night's call on Desktop PC"); prints the
/// room id. Lab servers only.
pub async fn headless_pad_seed() -> bool {
    use matrix_sdk::ruma::events::GlobalAccountDataEventType;
    let server = match crate::bridge::env_homeserver() {
        Ok(s) if lab_server(&s) => s,
        Ok(s) => {
            eprintln!("[padseed] FAIL refusing {s}: a lab server only (local or *.test)");
            return false;
        }
        Err(e) => {
            eprintln!("[padseed] FAIL {e}");
            return false;
        }
    };
    let (user, pass) = (std::env::var("TINHAT_USER").unwrap_or_default(), std::env::var("TINHAT_PASS").unwrap_or_default());
    let dir = std::env::temp_dir().join(format!("tinhat-padseed-{}", std::process::id()));
    let c = match lab_client(&server, &dir, &user, &pass, "Desktop PC").await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[padseed] FAIL {e}");
            return false;
        }
    };
    let _ = c.sync_once(crate::bridge::sync_settings()).await;
    if let Err(e) = c.encryption().bootstrap_cross_signing(None).await {
        eprintln!("[padseed] FAIL cross-signing (use a fresh account): {e}");
        return false;
    }
    let mi = |f: &str| format!("https://sounds.example.org/{f}");
    let favs = json!({"sounds": [
        {"name": "Bruh", "sourceUrl": mi("bruh.mp3"), "emoji": "😐", "duration": 1},
        {"name": "MLG Airhorn", "sourceUrl": mi("airhorn.mp3"), "emoji": "📯", "duration": 2},
        {"name": "Sad trombone", "sourceUrl": mi("sadtrombone.mp3"), "emoji": "🎺", "duration": 3},
        {"name": "Vine boom", "sourceUrl": mi("vine-boom.mp3"), "emoji": "💥", "duration": 1},
        {"name": "Emotional damage", "sourceUrl": mi("emotional-damage.mp3"), "emoji": "😭", "duration": 2},
        {"name": "Drum roll", "sourceUrl": mi("drumroll.mp3"), "emoji": "🥁", "duration": 4},
    ]});
    let raw = matrix_sdk::ruma::serde::Raw::from_json(serde_json::value::to_raw_value(&favs).expect("json"));
    if let Err(e) = c.account().set_account_data_raw(GlobalAccountDataEventType::from("dev.tinhat.soundboard_personal"), raw).await {
        eprintln!("[padseed] favourites: {e}");
    }
    let room = {
        use matrix_sdk::ruma::api::client::room::create_room::v3::Request as Create;
        let mut req = Create::new();
        req.name = Some("Game night".into());
        match c.create_room(req).await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[padseed] FAIL create room: {e}");
                return false;
            }
        }
    };
    for (i, (name, f, emoji)) in [("Victory fanfare", "fanfare.mp3", "🏆"), ("GG", "gg.mp3", "🎮"), ("Crickets", "crickets.mp3", "🦗")].iter().enumerate() {
        let content = json!({"name": name, "sourceUrl": mi(f), "emoji": emoji, "duration": 2, "addedAt": now_ms() - i as i64 * 1000});
        if let Err(e) = crate::appstate::send_state(&room, "dev.tinhat.soundboard", &format!("padseed-{i}"), content).await {
            eprintln!("[padseed] room sound: {e}");
        }
    }
    if let Err(e) = crate::rtc::join_rtc_session(&room, "https://lab.test/livekit/jwt").await {
        eprintln!("[padseed] FAIL call membership: {e}");
        return false;
    }
    let _ = c.sync_once(crate::bridge::sync_settings()).await;
    println!("[padseed] room {} device {}", room.room_id(), c.device_id().map(|d| d.to_string()).unwrap_or_default());
    let _ = std::fs::remove_dir_all(&dir);
    true
}
