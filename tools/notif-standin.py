#!/usr/bin/env python3
"""notif-standin.py — a stand-in org.freedesktop.Notifications server for the notification-action gates
(docs/NOTIF-ACTIONS.md, tools/README-gates.md "Notification actions"). Run it on a PRIVATE session bus
(dbus-run-session), never on a desktop's own bus.

  STANDIN_CAPS  comma list returned by GetCapabilities (default: the KDE Plasma set incl. inline-reply)
  STANDIN_LOG   JSON lines: every Notify / CloseNotification call and every signal fired

Notify(...) answers ids 1, 2, 3, … and logs {op, id, app, summary, body, actions, hints}. The gate fires the user's
part through a control interface on the same connection, so the signals come from the name's real owner:

  gdbus call --session --dest org.freedesktop.Notifications --object-path /dev/tinhat/StandIn \
             --method dev.tinhat.StandIn.Fire ID KEY TEXT
      KEY "reply-text"  → NotificationReplied(ID, TEXT), then NotificationClosed(ID, 2)   (KDE inline reply)
      KEY other         → ActionInvoked(ID, KEY), then NotificationClosed(ID, 2)
  dev.tinhat.StandIn.FireForeign ID KEY TEXT   the same signals from a SECOND connection that does not own the
      name (a negative control: the app must ignore them)
  dev.tinhat.StandIn.SetLocked BOOL   with STANDIN_SCREENSAVER=1 it also owns org.freedesktop.ScreenSaver and answers
      GetActive with this (the screen lock the app checks before an inline reply)
"""
import json
import os
import sys
import time

from gi.repository import Gio, GLib

CAPS = [c for c in os.environ.get("STANDIN_CAPS", "actions,body,body-hyperlinks,body-markup,body-images,icon-static,inline-reply,persistence").split(",") if c]
LOG = os.environ.get("STANDIN_LOG", "/tmp/notif-standin.jsonl")

XML = """
<node>
  <interface name="org.freedesktop.Notifications">
    <method name="GetCapabilities"><arg type="as" direction="out"/></method>
    <method name="Notify">
      <arg type="s" direction="in"/><arg type="u" direction="in"/><arg type="s" direction="in"/>
      <arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="as" direction="in"/>
      <arg type="a{sv}" direction="in"/><arg type="i" direction="in"/><arg type="u" direction="out"/>
    </method>
    <method name="CloseNotification"><arg type="u" direction="in"/></method>
    <method name="GetServerInformation">
      <arg type="s" direction="out"/><arg type="s" direction="out"/><arg type="s" direction="out"/><arg type="s" direction="out"/>
    </method>
    <signal name="NotificationClosed"><arg type="u"/><arg type="u"/></signal>
    <signal name="ActionInvoked"><arg type="u"/><arg type="s"/></signal>
    <signal name="NotificationReplied"><arg type="u"/><arg type="s"/></signal>
  </interface>
  <interface name="dev.tinhat.StandIn">
    <method name="Fire"><arg type="u" direction="in"/><arg type="s" direction="in"/><arg type="s" direction="in"/></method>
    <method name="FireForeign"><arg type="u" direction="in"/><arg type="s" direction="in"/><arg type="s" direction="in"/></method>
    <method name="SetLocked"><arg type="b" direction="in"/></method>
  </interface>
  <interface name="org.freedesktop.ScreenSaver">
    <method name="GetActive"><arg type="b" direction="out"/></method>
  </interface>
</node>
"""

state = {"next": 1, "conn": None, "foreign": None, "locked": False}


def log(rec):
    rec["t"] = round(time.time(), 3)
    with open(LOG, "a", encoding="utf-8") as f:
        f.write(json.dumps(rec, ensure_ascii=False) + "\n")


def emit(conn, name, args):
    conn.emit_signal(None, "/org/freedesktop/Notifications", "org.freedesktop.Notifications", name, args)


def fire(conn, nid, key, text, who):
    if key == "reply-text":
        emit(conn, "NotificationReplied", GLib.Variant("(us)", (nid, text)))
        log({"op": "signal", "by": who, "signal": "NotificationReplied", "id": nid, "text": text})
    else:
        emit(conn, "ActionInvoked", GLib.Variant("(us)", (nid, key)))
        log({"op": "signal", "by": who, "signal": "ActionInvoked", "id": nid, "key": key})
    conn.flush_sync(None)
    emit(conn, "NotificationClosed", GLib.Variant("(uu)", (nid, 2)))
    log({"op": "signal", "by": who, "signal": "NotificationClosed", "id": nid, "reason": 2})


def hint_value(v):
    try:
        x = v.unpack()
    except Exception:  # noqa: BLE001
        return "?"
    return x if isinstance(x, (str, int, float, bool)) else "<%s>" % v.get_type_string()


def on_call(conn, sender, path, iface, method, params, inv):
    if method == "GetCapabilities":
        inv.return_value(GLib.Variant("(as)", (CAPS,)))
    elif method == "GetServerInformation":
        inv.return_value(GLib.Variant("(ssss)", ("tinhat-standin", "Tin Hat gates", "1", "1.2")))
    elif method == "Notify":
        app, replaces, icon, summary, body, actions, hints, timeout = params.unpack()
        nid = replaces or state["next"]
        if not replaces:
            state["next"] += 1
        log({"op": "Notify", "id": nid, "sender": sender, "app": app, "summary": summary, "body": body, "actions": actions,
             "hints": {k: hint_value(params.get_child_value(6).lookup_value(k, None)) for k in hints}, "timeout": timeout})
        inv.return_value(GLib.Variant("(u)", (nid,)))
    elif method == "CloseNotification":
        (nid,) = params.unpack()
        log({"op": "CloseNotification", "id": nid, "sender": sender})
        emit(conn, "NotificationClosed", GLib.Variant("(uu)", (nid, 3)))
        inv.return_value(None)
    elif method == "Fire":
        nid, key, text = params.unpack()
        fire(conn, nid, key, text, "owner")
        inv.return_value(None)
    elif method == "SetLocked":
        (state["locked"],) = params.unpack()
        log({"op": "SetLocked", "locked": state["locked"]})
        inv.return_value(None)
    elif method == "GetActive":
        log({"op": "GetActive", "sender": sender, "answer": state["locked"]})
        inv.return_value(GLib.Variant("(b)", (state["locked"],)))
    elif method == "FireForeign":
        nid, key, text = params.unpack()
        if state["foreign"] is None:
            addr = Gio.dbus_address_get_for_bus_sync(Gio.BusType.SESSION, None)
            state["foreign"] = Gio.DBusConnection.new_for_address_sync(
                addr, Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT | Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION, None, None)
        fire(state["foreign"], nid, key, text, "foreign")
        inv.return_value(None)


def main():
    node = Gio.DBusNodeInfo.new_for_xml(XML)
    loop = GLib.MainLoop()

    def on_bus(conn, name):
        state["conn"] = conn
        for iface in node.interfaces:
            if iface.name == "org.freedesktop.ScreenSaver":
                if os.environ.get("STANDIN_SCREENSAVER") == "1":
                    conn.register_object("/ScreenSaver", iface, on_call, None, None)
                continue
            path = "/org/freedesktop/Notifications" if iface.name == "org.freedesktop.Notifications" else "/dev/tinhat/StandIn"
            conn.register_object(path, iface, on_call, None, None)
        if os.environ.get("STANDIN_SCREENSAVER") == "1":
            Gio.bus_own_name_on_connection(conn, "org.freedesktop.ScreenSaver", Gio.BusNameOwnerFlags.NONE, None, None)

    def on_name(conn, name):
        log({"op": "ready", "caps": CAPS})
        print("standin: owns org.freedesktop.Notifications", flush=True)

    def on_lost(conn, name):
        print("standin: could not own the name", file=sys.stderr, flush=True)
        loop.quit()

    Gio.bus_own_name(Gio.BusType.SESSION, "org.freedesktop.Notifications", Gio.BusNameOwnerFlags.NONE, on_bus, on_name, on_lost)
    loop.run()


if __name__ == "__main__":
    main()
