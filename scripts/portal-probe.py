#!/usr/bin/python3
"""Probe how far the GlobalShortcuts portal lets us get with a given app id.

Stops before BindShortcuts on purpose: that is the call that shows a dialog, and a
dismissed dialog is remembered permanently for the app id.
"""
import sys
import gi
gi.require_version("Gio", "2.0")
from gi.repository import Gio, GLib

APP_ID = sys.argv[1] if len(sys.argv) > 1 else "io.github.zulucode_design.SecondBrain"
BUS = "org.freedesktop.portal.Desktop"
PATH = "/org/freedesktop/portal/desktop"

bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
unique = bus.get_unique_name()
print(f"app id      : {APP_ID}")
print(f"bus name    : {unique}")

# 1. Registry.Register — required for non-sandboxed apps since xdg-desktop-portal 1.20,
#    and it must happen before any other portal call.
try:
    bus.call_sync(BUS, PATH, "org.freedesktop.host.portal.Registry", "Register",
                  GLib.Variant("(sa{sv})", (APP_ID, {})), None,
                  Gio.DBusCallFlags.NONE, 5000, None)
    print("Register    : OK")
except GLib.Error as e:
    print(f"Register    : FAILED -> {e.message}")

# 2. GlobalShortcuts.CreateSession — this is where GNOME validates the app id.
sender = unique[1:].replace(".", "_")
token = "probe0"
req_path = f"/org/freedesktop/portal/desktop/request/{sender}/{token}"

loop = GLib.MainLoop()
result = {}

def on_response(conn, s, p, i, sig, params):
    code, results = params.unpack()
    result["code"] = code
    result["results"] = results
    loop.quit()

bus.signal_subscribe(BUS, "org.freedesktop.portal.Request", "Response", req_path,
                     None, Gio.DBusSignalFlags.NONE, on_response)

try:
    bus.call_sync(BUS, PATH, "org.freedesktop.portal.GlobalShortcuts", "CreateSession",
                  GLib.Variant("(a{sv})", ({
                      "handle_token": GLib.Variant("s", token),
                      "session_handle_token": GLib.Variant("s", "probesess0"),
                  },)), None, Gio.DBusCallFlags.NONE, 5000, None)
except GLib.Error as e:
    print(f"CreateSession: FAILED at call -> {e.message}")
    sys.exit(1)

GLib.timeout_add_seconds(10, lambda: (result.setdefault("code", "timeout"), loop.quit())[1])
loop.run()

code = result.get("code")
if code == 0:
    print(f"CreateSession: OK -> {result['results'].get('session_handle')}")
    print("\n>>> app id ACCEPTED. Dev loop is viable.")
else:
    print(f"CreateSession: response code {code} (0=ok 1=cancelled 2=other)")
    print("\n>>> app id REJECTED or session refused.")
