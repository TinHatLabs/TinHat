#!/bin/bash
# linux-start-gate.sh BINARY [OUTDIR] — the Linux .bin start gate (12.1.16). Run it INSIDE a freshly updated system
# (a throwaway Arch container after `pacman -Syu` with the .bin's runtime packages, BUILDING.md's list). It checks, on
# that system:
#   1. needs:  no libMpvQt in the binary's NEEDED list or in its resolved library closure (that library's file name
#              changes between mpvqt versions: 12.1.11 - 12.1.13 needed libMpvQt.so.3 and stopped starting when Arch
#              moved to .so.4), and libmpv.so.2 resolves (it stays a shared library);
#   2. ldd:    every shared library the binary needs resolves (0 "not found");
#   3. start:  a real start under its own Xvfb with an empty profile reaches the sign-in screen: the app's own log line
#              "[boot] revealing main window: no session to restore", then a viewable "Tin Hat" window that has drawn
#              something (8+ colours), and 10 s later the app still running and the SAME window still viewable and
#              drawn (8+ colours; that capture is kept as signin.xwd).
#              No sign-in, no account, no sound server (PULSE_SERVER points nowhere).
# Started as root it needs GATE_USER=<a normal user> (the app runs as that user); started as a user it runs as itself.
# Needs Xvfb, xwininfo + xwd (xorg-xwininfo, xorg-xwd), readelf (binutils), setsid/setpriv (util-linux).
# GATE_SECONDS (default 120) = how long the start may take. Exit 0 = PASS; 1 = FAIL; 2 = the gate couldn't run.
set -u
B=${1:?usage: linux-start-gate.sh BINARY [OUTDIR]}
OUT=${2:-$(mktemp -d /tmp/linux-start-gate.XXXXXX)}
SECS=${GATE_SECONDS:-120}
case "$SECS" in ''|*[!0-9]*) echo "linux-start-gate: GATE_SECONDS must be a number" >&2; exit 2;; esac
[ -f "$B" ] && [ -r "$B" ] || { echo "linux-start-gate: no readable binary $B" >&2; exit 2; }
for t in Xvfb xwininfo xwd readelf ldd setsid setpriv od sort; do
  command -v "$t" >/dev/null || { echo "linux-start-gate: $t is missing" >&2; exit 2; }
done
mkdir -p "$OUT" || exit 2
B=$(readlink -f "$B")
SYS_PATH=/usr/local/bin:/usr/bin:/bin
F=0
pass() { echo "PASS $*"; }
fail() { echo "FAIL $*"; F=1; }

# who runs the app
RUNAS=()
if [ "$(id -u)" = 0 ]; then
  [ -n "${GATE_USER:-}" ] || { echo "linux-start-gate: started as root: set GATE_USER to a normal user" >&2; exit 2; }
  U_ID=$(id -u "$GATE_USER" 2>/dev/null) || { echo "linux-start-gate: no user $GATE_USER" >&2; exit 2; }
  [ "$U_ID" != 0 ] || { echo "linux-start-gate: GATE_USER must not be root" >&2; exit 2; }
  RUNAS=(setpriv --reuid="$GATE_USER" --regid="$(id -g "$GATE_USER")" --init-groups --)
fi
"${RUNAS[@]}" test -x "$B" || { echo "linux-start-gate: the app's user can't run $B" >&2; exit 2; }

# everything this run starts or makes, cleaned up on any exit. No job control: setsid then never forks, so $! is the
# app itself and also its process group id (APP_PGID, kept even after the app is gone so its helpers are reached).
set +m
XV=""; APP=""; APP_PGID=""; H=""; R=""
cleanup() {
  if [ -n "$APP" ] && kill -0 "$APP" 2>/dev/null; then
    kill -TERM -- "-$APP_PGID" 2>/dev/null
    for i in $(seq 1 30); do kill -0 "$APP" 2>/dev/null || break; sleep 0.5; done
    kill -0 "$APP" 2>/dev/null && { kill -KILL -- "-$APP_PGID" 2>/dev/null; echo "   (the app ignored SIGTERM for 15 s: killed)"; }
  fi
  [ -n "$APP_PGID" ] && kill -KILL -- "-$APP_PGID" 2>/dev/null   # helpers left in its process group (WebEngine), if any
  [ -n "$APP" ] && wait "$APP" 2>/dev/null
  APP=""; APP_PGID=""
  [ -n "$XV" ] && { kill "$XV" 2>/dev/null; wait "$XV" 2>/dev/null; }
  [ -n "$H" ] && rm -rf -- "$H"
  [ -n "$R" ] && rm -rf -- "$R"
}
trap cleanup EXIT
trap 'exit 2' INT TERM

echo "== system: $(. /etc/os-release 2>/dev/null; echo "${PRETTY_NAME:-unknown}"), kernel $(uname -r)"
command -v pacman >/dev/null && pacman -Q glibc qt6-base qt6-webengine mpvqt mpv kirigami 2>&1 | sed 's/^/   /'
echo "   libMpvQt files: $(ls /usr/lib/libMpvQt.so.* 2>/dev/null | tr '\n' ' ')"
echo "== binary: $B ($(sha256sum "$B" | cut -c1-64))"

# 1 + 2: what the binary needs, resolved with a clean environment (no LD_LIBRARY_PATH from the caller)
readelf -d "$B" >"$OUT/readelf.txt" 2>&1 || { echo "linux-start-gate: readelf failed" >&2; exit 2; }
grep -q '(NEEDED)' "$OUT/readelf.txt" || { echo "linux-start-gate: readelf listed no NEEDED entries (not a dynamic ELF?)" >&2; exit 2; }
env -i PATH="$SYS_PATH" ldd "$B" >"$OUT/ldd.txt" 2>&1; lrc=$?
direct=$(grep '(NEEDED)' "$OUT/readelf.txt" | grep -o 'libMpvQt[^]]*' | tr '\n' ' ')
closure=$(grep -o '[^[:space:]]*libMpvQt[^[:space:]]*' "$OUT/ldd.txt" | sort -u | tr '\n' ' ')
if [ -n "$direct" ] || [ -n "$closure" ]; then
  fail "needs: libMpvQt is a shared dependency (NEEDED: ${direct:-none}; resolved closure: ${closure:-none}) — link mpvqt statically: scripts/mpvqt-static.sh"
elif ! grep -q '^[[:space:]]*libmpv\.so\.2 => /' "$OUT/ldd.txt"; then
  fail "needs: libmpv.so.2 does not resolve as a shared library"
else
  pass "needs: no libMpvQt (NEEDED or closure); libmpv.so.2 => $(grep '^[[:space:]]*libmpv\.so\.2 =>' "$OUT/ldd.txt" | awk '{print $3}')"
fi
nf=$(grep -c 'not found' "$OUT/ldd.txt")
if [ "$nf" -gt 0 ]; then
  fail "ldd: $nf not found: $(grep 'not found' "$OUT/ldd.txt" | awk '{print $1}' | tr '\n' ' ')"
elif [ $lrc -ne 0 ]; then
  fail "ldd: exit $lrc ($(head -n 2 "$OUT/ldd.txt" | tr '\n' ' '))"
else
  pass "ldd: $(grep -c '=>' "$OUT/ldd.txt") libraries, 0 not found"
fi

# 3: start. A profile + runtime dir nobody else uses, owned by the app's user.
H=$(mktemp -d /tmp/linux-start-gate-home.XXXXXX) && R=$(mktemp -d /tmp/linux-start-gate-run.XXXXXX) || exit 2
chmod 700 "$H" "$R"; [ ${#RUNAS[@]} -gt 0 ] && chown "$GATE_USER": "$H" "$R"
# Xvfb picks a free display itself and says which (-displayfd), so this run never looks at another X server
: >"$OUT/xvfb.display" || exit 2   # emptied here, before Xvfb starts, so an old run's number is never read
Xvfb -displayfd 3 -screen 0 1600x1000x24 -nolisten tcp 3>>"$OUT/xvfb.display" >"$OUT/xvfb.log" 2>&1 & XV=$!
for i in $(seq 1 50); do [ -s "$OUT/xvfb.display" ] && break; kill -0 "$XV" 2>/dev/null || break; sleep 0.2; done
DN=$(tr -dc 0-9 <"$OUT/xvfb.display" 2>/dev/null)
[ -n "$DN" ] && kill -0 "$XV" 2>/dev/null || { echo "linux-start-gate: Xvfb did not start ($(tail -n 2 "$OUT/xvfb.log" | tr '\n' ' '))" >&2; exit 2; }
D=":$DN"
xwininfo -display "$D" -root >/dev/null 2>&1 || { echo "linux-start-gate: Xvfb on $D doesn't answer" >&2; exit 2; }
# env -i: only what a desktop session would give it. setsid: the app leads its own process group, so cleanup reaches
# its helpers; setsid (no job control) and setpriv exec in place, so $APP is the app itself.
setsid "${RUNAS[@]}" env -i HOME="$H" XDG_RUNTIME_DIR="$R" DISPLAY="$D" QT_QPA_PLATFORM=xcb PATH="$SYS_PATH" \
  LANG=C.UTF-8 TINHAT_NO_LOG_REDIRECT=1 TINHAT_NO_SINGLE_INSTANCE=1 PULSE_SERVER=unix:/nonexistent \
  "$B" >"$OUT/app.log" 2>&1 < /dev/null & APP=$!; APP_PGID=$APP
viewable() { xwininfo -display "$D" -id "$1" 2>/dev/null | grep -q 'Map State: IsViewable'; }
be32() { # be32 FILE BYTE_OFFSET -> the big-endian 32-bit header field there (xwd writes its header MSB first)
  od -An -tu1 -j "$2" -N4 "$1" | awk '{print ((($1 * 256 + $2) * 256 + $3) * 256 + $4)}'
}
colours_of() { # colours_of WINDOW -> the distinct pixel values drawn in it (32 bits per pixel, as Xvfb's depth 24 is)
  xwd -display "$D" -id "$1" -silent >"$OUT/signin.xwd" 2>/dev/null || { echo 0; return; }
  local hs bpp nc
  hs=$(be32 "$OUT/signin.xwd" 0); bpp=$(be32 "$OUT/signin.xwd" 44); nc=$(be32 "$OUT/signin.xwd" 76)
  [ "$bpp" = 32 ] || { echo 0; return; }
  # pixels start after the header and the colour map (12 bytes per entry)
  tail -c +$(( hs + 12 * nc + 1 )) "$OUT/signin.xwd" | od -An -v -tx4 -w4 | sort -u | wc -l
}
find_win() { # the viewable top-level windows named exactly "Tin Hat"
  local id
  for id in $(xwininfo -display "$D" -root -tree 2>/dev/null | grep -E '^[[:space:]]*0x[0-9a-f]+ "Tin Hat":' | awk '{print $1}'); do
    viewable "$id" && { echo "$id"; return 0; }
  done
  return 1
}
started=$(date +%s); reveal=""; win=""; wid=""; colours=0
# until the deadline: the sign-in log line, then a viewable Tin Hat window, then 8+ colours drawn in it
while [ $(( $(date +%s) - started )) -lt "$SECS" ]; do
  kill -0 "$APP" 2>/dev/null || break
  [ -z "$reveal" ] && grep -aqF '[boot] revealing main window: no session to restore' "$OUT/app.log" && reveal=$(( $(date +%s) - started ))
  if [ -n "$reveal" ] && { [ -z "$wid" ] || ! viewable "$wid"; }; then wid=$(find_win) && win=$(( $(date +%s) - started )); fi
  if [ -n "$wid" ]; then colours=$(colours_of "$wid"); [ "$colours" -ge 8 ] && break; fi
  sleep 0.5
done
still=0; geom=""; colours2=0
if [ -n "$wid" ] && [ "$colours" -ge 8 ]; then
  geom=$(xwininfo -display "$D" -id "$wid" 2>/dev/null | awk '/Width:/ {w=$2} /Height:/ {h=$2} END {print w "x" h}')
  sleep 10
  # 10 s on: still running, the same window still viewable and still drawn. This capture is the evidence screenshot
  # (the first one can still show the start-up window over the sign-in card).
  if kill -0 "$APP" 2>/dev/null && viewable "$wid"; then colours2=$(colours_of "$wid"); [ "$colours2" -ge 8 ] && still=1; fi
fi
if [ -n "$wid" ] && [ "$colours" -ge 8 ] && [ $still = 1 ]; then
  pass "start: the sign-in screen after ${reveal} s (window $wid ${geom} drawn by ${win} s, $colours colours), the app running and the same window viewable and drawn 10 s later ($colours2 colours)"
else
  alive=1; kill -0 "$APP" 2>/dev/null || { wait "$APP" 2>/dev/null; st=$?; alive=0; }
  if [ -z "$reveal" ] && [ $alive = 0 ]; then why="the app exited (status $st) before the sign-in screen"
  elif [ -z "$reveal" ]; then why="no sign-in log line within ${SECS} s"
  elif [ -z "$wid" ]; then why="the sign-in log line came, but no viewable Tin Hat window"
  elif [ "$colours" -lt 8 ]; then why="the window drew nothing within ${SECS} s ($colours colours)"
  else why="10 s after the sign-in screen the app, its window or its drawing was gone ($colours2 colours)"; fi
  [ -n "$reveal" ] && [ $alive = 0 ] && why="$why; the app exited with status $st"
  fail "start: $why; the app's last lines:"
  tail -n 8 "$OUT/app.log" | cut -c1-200 | sed 's/^/     /'
fi
cleanup; XV=""; H=""; R=""
echo "   evidence: $OUT (app.log, ldd.txt, readelf.txt$( [ -s "$OUT/signin.xwd" ] && echo ', signin.xwd'))"
if [ $F = 0 ]; then echo "LINUX-START-GATE: PASS"; else echo "LINUX-START-GATE: FAIL"; fi
exit $F
