# MEDIA_PIPELINE.md — GStreamer media port (Linux/KDE → Windows)

Part of the Windows handoff set. Start at [`../HANDOFF_WINDOWS.md`](../HANDOFF_WINDOWS.md).
Sibling docs (do not duplicate — cross-referenced inline):
[`PLATFORM_AUDIT.md`](PLATFORM_AUDIT.md) ·
[`BUILD_WINDOWS.md`](BUILD_WINDOWS.md) ·
[`PACKAGING.md`](PACKAGING.md) ·
[`UPDATE_PATH.md`](UPDATE_PATH.md) ·
[`TEST_PLAN.md`](TEST_PLAN.md)

**Scope of THIS doc:** the GStreamer capture/render/encode stack only. Build-system
discovery (pkg-config, Qt/KF6 paths), DLL bundling layout, and the self-updater live in
the sibling docs above. This doc tells you, element-by-element, how each pipeline changes
and what is genuinely risky.

**Hard rule:** documentation/design only. Do **not** edit any existing `crates/streamer/**`
or `crates/app/**` source/build file from a Linux desktop's checkout. The Windows-VM
Claude does the actual `cfg(target_os)` refactor against the cloned repo.

---

## 0. The big picture: what ports for free and what doesn't

The media path splits into two halves:

- **(A) GStreamer capture + render** — `crates/streamer/src/{capture,camera,call}.rs`,
  `crates/streamer/src/lk_publish.rs`, and the app-side C++ sink shim
  `crates/app/src/video_bridge.cpp`. **This is the entire Windows port surface.**
- **(B) libwebrtc encode/encrypt/transport** — the real E2EE publish/subscribe path.
  The app pushes **raw I420 / raw F32 audio frames** into libwebrtc's
  `NativeVideoSource`/`NativeAudioSource` and pulls remote frames from
  `NativeVideoStream`/`NativeAudioStream`. libwebrtc swaps its own Audio Device Module
  (Linux `libpulse` → Windows **WASAPI**) inside the prebuilt blob, **and we bypass the
  ADM anyway** by pushing/pulling frames through GStreamer. So device handling is 100%
  in our GStreamer code, not in libwebrtc.

**Consequence #1:** `libpulse` is a non-issue on Windows. The prebuilt win-x86_64
libwebrtc carries a WASAPI ADM; no source change, no `libpulse.dll` to ship.

**Consequence #2 (the one painful exception):** NVENC. libwebrtc's NVENC factory is
**Linux-gated in `webrtc-sys`** — `CUDA_HOME` on Windows is a no-op and the headline
hardware screenshare **silently falls back to software VP9/OpenH264**. See
[§7](#7-nvenc-the-headline-feature-does-not-port-for-free) — this is verification #5 and
it is load-bearing. Do not promise hardware NVENC on Windows in the first build.

### Capture-source mapping at a glance

| Concern | Linux element | Windows element | Risk |
|---|---|---|---|
| Screen capture | `pipewiresrc fd= path=` (xdg portal) | `d3d11screencapturesrc` (DXGI/WGC) | high — no fd/node model; needs native picker |
| Camera | `v4l2src device=/dev/videoN` | `mediafoundationvideosrc device-path=` (or `ksvideosrc`) | medium |
| Mic capture | `pipewiresrc target-object=` | `wasapisrc` / `wasapi2src` | medium |
| Remote playback | `pipewiresink target-object=` | `wasapisink` / `wasapi2sink` | low |
| "What's playing" loopback | `pipewiresrc target-object="<sink>.monitor"` | `wasapisrc loopback=true device=<render-id>` | medium |
| Device enumeration | `DeviceMonitor` keyed `node.name`/`device.path` | `DeviceMonitor` keyed WASAPI/MF device id | medium |
| Default endpoint | `pactl get-default-{source,sink}` | Core Audio `IMMDeviceEnumerator` or "" = default | low |
| Video → QML tiles | `qml6glsink` (+`glupload`/`glcolorconvert`) | **primary: appsink → `QVideoSink`** (see §6) | high |
| HW H.264 encode (publish) | libwebrtc NVENC factory (CUDA) | **software VP9/OpenH264** (NVENC not built on Win) | high |
| Noise cancel | `audiornnoise` | `audiornnoise` (same, bundle the DLL) | trivial |
| URL/file audio decode | `uridecodebin` | `uridecodebin` (bundle `souphttpsrc`+decoders) | low |

GStreamer target: **1.28.x official MSVC x86_64 build** (current stable 1.28.4). Install
the **devel** components on the build VM; ship a curated runtime subset (see
[`PACKAGING.md`](PACKAGING.md) and [§8](#8-plugin--dll-deployment)). The `gstreamer`
0.23 Rust bindings in this workspace are forward-compatible with a 1.28 runtime. Use the
**MSVC** GStreamer build (not MinGW) to match the Rust MSVC toolchain — unless you adopt
the MSYS2/UCRT64 route discussed in [`BUILD_WINDOWS.md`](BUILD_WINDOWS.md), in which case
keep the whole toolchain MinGW-consistent. **Do not mix MinGW GStreamer with an MSVC app.**

---

## 1. Screen capture: portal/`pipewiresrc` → `d3d11screencapturesrc`

### 1.1 What Linux does today

`crates/streamer/src/capture.rs` opens the KDE/Wayland xdg-desktop-portal **ScreenCast**
session via `ashpd` (`pick_screencast()`), which pops the compositor's source picker
(Monitor|Window, `CursorMode::Embedded`, persist `DoNot`) and returns a **PipeWire fd +
node id + size**. That `ScreenCast { fd, node_id, size, session }` handle is fed to
`pipewiresrc fd=<fd> path=<node_id>` in `lk_publish.rs::build_capture_pipeline`. The
handle MUST be held for the stream lifetime — dropping the fd tears down the PipeWire
remote and EOSes the pipeline; `ScreenCast::close()` closes the portal session.

The capture pipeline (`lk_publish.rs`, exact current text):

```text
pipewiresrc fd={fd} path={node_id} do-timestamp=true keepalive-time=1000 ! \
videorate drop-only=true ! \
video/x-raw(ANY),framerate={fps}/1 ! \
videoconvert ! videoscale add-borders=false name=vscale ! \
capsfilter name=outcaps caps=video/x-raw,format=I420 ! \
appsink name=sink emit-signals=false max-buffers=2 drop=true sync=false
```

The **anti-game-hitch trick** (read the long comment in `build_capture_pipeline` before
touching this): `videorate drop-only=true` caps the framerate **before** any pixel touch,
and the capsfilter uses `video/x-raw(ANY)` (the `(ANY)` caps-feature) so the GPU memory
feature (`memory:DMABuf` on Linux) is **preserved through `videorate`**, deferring the
single unavoidable GPU→CPU readback (`videoconvert`, needed for libwebrtc's CPU I420
source) to run only at `fps`. A featureless `video/x-raw,framerate=` caps would match only
system memory and either fail to link or push the full-rate readback upstream — defeating
the fix. A one-shot pad probe on `vscale`'s sink pad reads the negotiated source size and
`fit_even()`s the output caps.

### 1.2 Windows: there is no fd/node model

`d3d11screencapturesrc` (DXGI Desktop Duplication by default; `capture-api=wgc` for
Windows.Graphics.Capture, WGC added in 1.22) takes a **monitor/window selector**, not an
fd. There is no portal session lifecycle to keep alive. Key properties (verify with
`gst-inspect-1.0 d3d11screencapturesrc` on the VM):

- `monitor-index` (zero-based; `-1` = primary) or `monitor-handle` (HMONITOR)
- `window-handle` (HWND) for single-window capture; `window-capture-mode` (since 1.24:
  entire-window vs client-area)
- `show-cursor` (maps `CursorMode::Embedded`)
- `capture-api` (`dxgi` | `wgc`); `show-border` (WGC, Win11)
- `adapter` (GPU index)

It exposes a **`GstDeviceProvider`** (`Source/Monitor`), so a `DeviceMonitor` can
enumerate target monitors. WGC can also show the system `GraphicsCapturePicker` for a
portal-like UX.

Output buffers are **`memory:D3D11Memory`**, not DMABuf. Re-derive the (ANY) trick against
D3D11: keep `videorate drop-only=true` + `video/x-raw(ANY),framerate={fps}/1`, then
download with `d3d11download` (or `d3d11convert ! d3d11download`) so the one readback runs
at `fps`. Drop the `do-timestamp`/`keepalive-time` props (they are `pipewiresrc`-specific).
Read the source size from `QScreen` geometry or the negotiated caps instead of the portal's
`size()`.

Proposed Windows pipeline:

```text
d3d11screencapturesrc monitor-index={idx} show-cursor=true ! \
videorate drop-only=true ! \
video/x-raw(ANY),framerate={fps}/1 ! \
d3d11convert ! d3d11download ! \
videoconvert ! videoscale add-borders=false name=vscale ! \
capsfilter name=outcaps caps=video/x-raw,format=I420 ! \
appsink name=sink emit-signals=false max-buffers=2 drop=true sync=false
```

> Zero-copy alternative (defer until the software-encode baseline works): keep frames on
> the GPU into a D3D11/NVENC gst encoder (`nvd3d11h264enc`) instead of downloading to I420.
> But the **real publish path feeds raw I420 to libwebrtc's `NativeVideoSource`**, which
> re-encodes itself, so a GStreamer GPU encoder only helps if you also change the transport
> away from the libwebrtc SDK encode — out of scope for parity. See [§7](#7-nvenc-the-headline-feature-does-not-port-for-free).

### 1.3 The native picker (replaces the portal UI)

You must build what the compositor gave you for free. Options:

1. **WGC system picker** — `GraphicsCapturePicker` (Windows.Graphics.Capture, via the
   `windows` crate). Closest to the portal UX; returns a `GraphicsCaptureItem` you map to a
   monitor/window for `d3d11screencapturesrc`.
2. **Custom QML picker** — enumerate monitors (`EnumDisplayMonitors` / Qt `QScreen`) and
   windows (`EnumWindows`) and present a grid; pass the chosen `monitor-index`/`HWND`.

### 1.4 Proposed abstraction (design)

Replace the `ScreenCast` struct with a platform-agnostic trait so `lk_publish.rs` /
`call.rs` stop naming `pipewiresrc`. See `proposed_cfg_layout` in
[`PLATFORM_AUDIT.md`](PLATFORM_AUDIT.md); the streamer-side shape:

```rust
// crates/streamer/src/capture/mod.rs (design)
pub trait ScreenSource: Send {
    /// Returns a pipeline ending in `appsink name=sink`.
    fn build_capture_pipeline(&self, max_w: u32, max_h: u32, fps: u32) -> anyhow::Result<gst::Pipeline>;
    fn size(&self) -> Option<(u32, u32)>;
    fn close(self: Box<Self>) -> futures::future::BoxFuture<'static, ()>;
}
// capture/linux.rs   -> today's ashpd ScreenCast (pipewiresrc fd/path + DMABuf head)
// capture/windows.rs -> WgcScreenSource { monitor_idx | hwnd } (d3d11screencapturesrc + d3d11download)
// capture/mod.rs     -> pub async fn pick_screen() -> Result<Box<dyn ScreenSource>>  (#[cfg(target_os)])
```

`ashpd` becomes `#[cfg(target_os = "linux")]` only (it won't meaningfully link on Windows —
no D-Bus portal). See [`PLATFORM_AUDIT.md`](PLATFORM_AUDIT.md) for the Cargo gating.

### 1.5 Known Windows bug to plan around

`d3d11screencapturesrc` with `window-handle` can **stop producing buffers on Windows 11
24H2 when the captured window has no visual change** (reported Jan 2025, no confirmed fix
version). Mitigation: prefer **monitor** capture, or use `capture-api=wgc` for window
capture, and test on the VM's exact build. Desktop Duplication can also fail under a
Service/secure-desktop/UAC context — fine in a normal interactive session.

---

## 2. Camera: `v4l2src` → `mediafoundationvideosrc`

### 2.1 Today

`crates/streamer/src/camera.rs` uses `v4l2src device="/dev/videoN"`. `list_cameras()`
builds a `DeviceMonitor` filtered `Video/Source`, keyed on **`device.path`** (pure-v4l2
provider) with `api.v4l2.path` as fallback (PipeWire provider wraps the same camera).
Publish resolution `CAM_W=1280` `CAM_H=720`. The **r113 active-downscale fix** pins a 16:9 mode **larger** than output
(`CAP_MAX_W=1920`) so `videoscale` always actively downscales — pinning the exact 1280×720
output made `videoscale` a passthrough and that passthrough buffer crashed `qml6glsink`'s
`glupload`. Raw cams go `… ! videoconvert`; MJPEG-only cams go
`… ! image/jpeg ! jpegdec ! videoconvert`; **never `decodebin`** (it broke raw cams).

### 2.2 Windows

Swap the source to **`mediafoundationvideosrc`** (preferred) with `device-path=<MF
symbolic link>` (e.g. `\\?\usb#vid_xxxx&pid_yyyy...#{guid}\global`) or `device-index=N`
for a simple picker. `ksvideosrc device-index=` (the `winks` plugin) is the alternative if
a specific camera misbehaves under MF. The `DeviceMonitor` `Video/Source` pattern still
works — the MF provider (`mfdeviceprovider`) exposes each `GstDevice` with `device.path` =
the MF symbolic link string. The READY-state caps-probe (`probe_camera_caps`) and the
downscale logic are **source-agnostic** (they operate on negotiated caps) and carry over.

```text
mediafoundationvideosrc device-path="{mf_id}" ! image/jpeg{cap} ! jpegdec ! videoconvert ! ...
# or, raw:
mediafoundationvideosrc device-path="{mf_id}" ! videoconvert ! ...
```

**Gotchas:** (1) MF device ids are a totally different string format from `/dev/videoN`, so
any persisted Linux camera id won't carry over — `load_saved_device`'s validate-against-
live-list already tolerates a stale id, so it degrades to "first camera." (2) Some UVC cams
expose MJPEG-only high-res modes under MF, needing the `jpegdec` branch. (3) **Re-validate
the r113 camera-squish fix on the Windows GL/D3D backend** using the in-app
`TINHAT_CAM_GLTEST` self-test harness + `TINHAT_CAM_FORCE` override — the original crash was
NVIDIA/GL-context-specific and may behave differently with the §6 video sink choice. (4) The
Elgato 4K X (host-only on Linux today) will appear as an MF device on Windows — decide
whether to expose it.

---

## 3. Audio capture/playback: PipeWire → WASAPI

> **Architecture warning (honest):** the PipeWire **virtual-sink / filter-chain soundboard**
> (the Chat / Gaming / Virtual A / Virtual B sinks, AutoMic) has **no Windows equivalent**
> and is a real feature-parity gap, not a 1:1 element swap. See [§5](#5-soundboard--music-share--virtual-sinks-the-parity-gap).

All of these live in `crates/streamer/src/call.rs`.

### 3.1 Mic capture — `build_mic_pipeline()` / `build_mic_desc()`

Today (`pipewiresrc target-object=<node>` or bare):

```text
pipewiresrc target-object="{node}" ! audioconvert ! audioresample ! \
audio/x-raw,format=F32LE,rate=48000,channels=1 ! [audiornnoise] ! \
audiobuffersplit ! audiorate ! queue ! appsink name=...
```

This produces **exact 10 ms F32 frames** for libwebrtc's `NativeAudioSource` — the
crackle-fix feeder (`start_mic_capture` monotonic timer, `queue_size_ms=0` fast path).

Windows: replace the head with **`wasapisrc device=<id> low-latency=true`** (or
`wasapi2src`). Bare = default capture device. **`audioconvert` / `audioresample` /
`audiobuffersplit` / `audiorate` / `queue` / `appsink` / `audiornnoise` are all
cross-platform** — keep them verbatim. The exact-480-sample feeder logic is clock behaviour,
not Linux-specific, and ports unchanged. **Re-verify** the `audiobuffersplit` 1/100
re-blocking holds, because WASAPI's quantum differs from PipeWire's 256-sample quantum —
the whole point is to hand libwebrtc exactly 10 ms.

### 3.2 Remote playback — `build_playback()` / `set_playback_target()`

Today: `pipewiresink sync=false name=out target-object=<node>`, re-pointed live via the
NULL-cycle (NULL → set `target-object` → `sync_state_with_parent`) because `pipewiresink`
only re-reads the target on (re)connect.

Windows: **`wasapisink device=<id>`** (or `wasapi2sink`); empty `device` = default endpoint.
The **same NULL-cycle re-point pattern works**. `playback_sinks()` WeakRef registry and the
per-track `volume` element are portable; `appsrc`/`audioconvert`/`audioresample`/`volume`/
`queue` unchanged.

### 3.3 Device enumeration — `list_devices(class)`

Today: `DeviceMonitor.add_filter("Audio/Sink"|"Audio/Source")` keyed on **`node.name`**
(PipeWire-only). Windows: the WASAPI provider works, but key off the **WASAPI device id**
property — `node.name` is absent on Windows. Everything that stores `node.name`
(`mic_target`, `playback_target`, music-source files) must switch to the WASAPI device id
string. Wrap this in a `DeviceEnumerator` trait with `cfg` impls (Linux: `node.name` +
`pactl`; Windows: WASAPI/MF device id + Core Audio default).

### 3.4 Default endpoint — `default_audio_source()` / `default_audio_sink()`

Today these shell out to **`pactl get-default-source` / `get-default-sink`** (used to
exclude the user's own mic from share sources = anti-echo, and to tap the real default
sink for the output meter). **No `pactl` on Windows.** Query the Core Audio default
endpoint via `IMMDeviceEnumerator::GetDefaultAudioEndpoint` (the `windows` crate), **or**
treat empty target = default and resolve lazily. A tiny helper or "empty = default"
suffices.

### 3.5 libwebrtc ADM

Nothing to do. The prebuilt win-x86_64 libwebrtc uses the WASAPI ADM internally; the app
pushes/pulls raw frames so the ADM is bypassed anyway. The Linux↔Windows libpulse↔WASAPI
swap is invisible at the app level. (Verification: the `PlatformAudio` device enumeration
path in `vendor/livekit` is **not used today** — relevant only if you ever call it.)

---

## 4. "What's playing" loopback (output meter + music-share input)

`call.rs` `build_aux_pipewire_desc()` / `start_output_monitor` capture a sink's monitor:

```text
pipewiresrc target-object="{sink}.monitor" ! audioconvert ! audioresample ! ...
```

Every PipeWire `Audio/Sink` auto-exposes a `<node>.monitor` source. `list_audio_sources()`
synthesizes `<node>.monitor` strings; `audio_source_exists()` strips `.monitor` to validate
the sink (`call.rs` ~lines 875–938).

**Windows:** WASAPI loopback capture is the direct equivalent —
**`wasapisrc loopback=true device=<render-endpoint-id>`**. CRITICAL: loopback is on the
**original `wasapi` plugin only**; **`wasapi2src` does NOT implement loopback** (verified).
Use `wasapi` for loopback, `wasapi2` for the primary mic/playback paths if you want its
better routing. Replace the `.monitor` naming convention with a `loopback:<render-id>` tag
mapped to `wasapisrc loopback=true`.

**Parity gap to flag:** plain WASAPI loopback captures the **whole endpoint**, not a single
app's audio — per-app monitors (sharing just one game's sound) are **not** available this
way. Document this as a behaviour difference.

---

## 5. Soundboard / music-share / virtual sinks (the parity gap)

The cross-platform pieces port cleanly:

- **RNNoise** (`audiornnoise`, gst-plugins-bad / rsaudiofx) — same element on Windows; just
  bundle the DLL. The existing `ElementFactory::find("audiornnoise")` guard disables the
  feature gracefully if absent.
- **URL/file decode** (`uridecodebin` in `build_aux_audio_pipeline`, for soundboard +
  music URL/file) — cross-platform. Ensure `souphttpsrc` (or `curlhttpsrc`) + audio
  decoders (`mpg123`/`avdec_*`) are in the bundle. `file://` works as-is.

The architecture that does **not** port: the **PipeWire virtual-sink graph** (Chat /
Gaming / Virtual A / Virtual B) and PipeWire filter-chains. Windows has no PipeWire. Design
options, in order of effort:

1. **In-pipeline `audiomixer`** — mix the contributing sources inside GStreamer and emit
   one WASAPI sink. Most faithful, most work.
2. **WASAPI loopback + per-endpoint routing** — rely on Windows' own endpoint model.
3. **Lean on the RODECaster Pro II hardware mixing** — the user already routes through it
   ([`user_audio_setup`] in MEMORY); on Windows the soundboard could defer to the hardware
   device. Lowest effort, least software parity.

Pick explicitly and document it for the user — this is a feature decision, not a mechanical
port. Cross-check the soundboard test cases in [`TEST_PLAN.md`](TEST_PLAN.md).

---

## 6. Video → QML tiles: the `qml6glsink` question (HIGH RISK)

This is the single biggest unknown in the whole port. Read this section carefully;
verifications #2 and #5 **override** any optimistic earlier note.

### 6.1 What Linux does

`crates/app/src/video_bridge.cpp` renders remote tiles + local camera self-view into the
QML grid via **`qml6glsink` + `glupload` + `glcolorconvert`** sharing Qt's GL scenegraph
context (`GstGLQt6VideoItem`, QML type `org.freedesktop.gstreamer.Qt6GLVideoItem`). It
**forces the OpenGL RHI** (`QQuickWindow::setGraphicsApi(QSGRendererInterface::OpenGL)` in
`tinhat_video_preinit`) and pins GstGL to match Qt's context in `tinhat_pin_gstgl_for_qpa()`:
Wayland → `GST_GL_PLATFORM=egl` / `GST_GL_API=gles2`; xcb → unpinned (GLX). The camera
selfview tail is `… ! glupload ! glcolorconvert ! qml6glsink name=sink sync=false`.

### 6.2 Why "it works on Windows under the default backend" is FALSE

- **Qt6's default RHI on Windows is Direct3D 11**, not OpenGL. `qml6glsink` is a GL sink;
  under the default D3D11 backend the tile is **black**. You would have to force the OpenGL
  RHI (`QSG_RHI_BACKEND=opengl` or `setGraphicsApi(OpenGL)` — the app already does the
  latter) for it to render at all.
- **`qml6glsink` ships in NO upstream GStreamer Windows binary, on any OS/version** (still
  true in 1.28.x) — the project bundles no Qt/qml plugins (Qt is heavy + licensing). You
  must **build `gstqml6.dll` yourself** from `gst-plugins-good/ext/qt6` against the **exact
  same Qt6** (minor version + MSVC ABI + matching `gstreamer-1.0` version) the app links.
  A Qt-minor mismatch silently breaks the sink or crashes on registration.
- Even with OpenGL forced and a custom `gstqml6.dll`, documented Windows reliability risks:
  - GstGL ↔ WGL context-sharing fails on some drivers — **GStreamer's own docs call out the
    Intel driver on Windows** (exactly the friends' Intel-iGPU population).
  - Known **Qt6/Win11 bug: two `GstGLQt6VideoItem` in one scene → one continuously flashes
    green** — directly hits the multi-tile call grid + self-view.
  - Forcing desktop GL falls back to `opengl32sw.dll` (Mesa llvmpipe, software) on machines/
    VMs lacking a hardware GL2 ICD → software/black in a GPU-less VM.

### 6.3 Recommended PRIMARY Windows path: appsink → `QVideoSink` → QML `VideoOutput`

Make the **`appsink` → `QVideoFrame` → `QVideoSink` → QML `VideoOutput`** path the primary
Windows video-display strategy, **not a fallback**:

- **Backend-agnostic** — runs under the default D3D11 RHI; no need to force OpenGL (which
  itself destabilizes the rest of the Qt6-on-Windows UI).
- **No custom `gstqml6.dll`** to build/ABI-match.
- **Sidesteps GstGL context-sharing entirely** and the green-flash multi-tile bug.
- Capture/encode stays in GStreamer exactly as designed (§1–§2); **only the display leg**
  changes from `qml6glsink` to `appsink`. Qt Multimedia does the RHI upload internally.

Threading rule: **`QVideoSink::setVideoFrame()` must be marshalled to the GUI thread**
(mirrors the current GUI-thread affinity of the `qml6glsink` attach/detach C++ shim).

Keep the C++ shim's `extern "C"` ABI stable (`tinhat_video_*`, `tinhat_apply_color_scheme`,
`tinhat_mpv_register`, `tinhat_pin_gstgl_for_qpa`) so `crates/app/src/video.rs` needs zero
changes — only the **bodies** swap. Design a `VideoSink` strategy enum:

```text
GlQt6        = qml6glsink + glupload/glcolorconvert + pin_gstgl_for_qpa   (Linux default)
AppsinkTex   = appsink -> QVideoFrame -> QVideoSink (QML VideoOutput)      (Windows default)
D3d11Window  = d3d11videosink in a separate HWND                          (NOT in-scene; see below)
```

### 6.4 Do NOT use `d3d11videosink` for in-scene tiles

`d3d11videosink` renders to its **own HWND/swapchain**, not into a `QQuickItem`. It cannot
composite into the Qt Quick scene graph the way `qml6glsink` does. It is at most a single
full-screen overlay window — unsuitable for Tin Hat's tiled grid + self-view + bubble
morph. (Qt forum reached no in-scene solution.) List it only as a last-ditch single-window
option.

### 6.5 If someone still wants `qml6glsink` on Windows

Scope it as a **gated experiment**, not the default: build `gstqml6.dll` from
`gst-plugins-good/ext/qt6` (meson `-Dqt6=enabled` with `QMAKE6` pointing at the app's
`qmake6.exe`, **or** Cerbero with the qt6 variant), force the OpenGL RHI, drop the DLL into
the bundle's `lib/gstreamer-1.0`, and **prove multi-tile + Intel-iGPU rendering** before
relying on it. Add the Windows branch to `tinhat_pin_gstgl_for_qpa()`:
`platformName()=="windows"` → `GST_GL_PLATFORM=wgl` + `GST_GL_API=opengl` (or `egl`/`gles2`
if Qt runs on ANGLE). Debug with `GST_DEBUG=glcontext:5,qml6glsink:5`.

### 6.6 Watch-party (mpv) surface

Separate from the call tiles. `MpvAbstractItem` (mpvqt) is **OpenGL-only** (it hard-checks
`QQuickWindow::graphicsApi() == OpenGL`). Build details, the `setGraphicsApi(OpenGL)`
requirement, yt-dlp bundling, and the toolchain-ABI constraint live in
[`BUILD_WINDOWS.md`](BUILD_WINDOWS.md) and [`PACKAGING.md`](PACKAGING.md). If you go the
appsink/D3D11 route for call tiles and **don't** force OpenGL globally, the mpv watch-party
needs its own OpenGL window or must be deferred — flag this conflict (see Open questions).

---

## 7. NVENC: the headline feature does NOT port for free

> **2026-07-05 update (see HANDOFF §0):** `webrtc-sys` is now a **vendored in-repo fork**
> (`vendor/webrtc-sys/`, via `[patch.crates-io]`) carrying NVENC encoder pinning, an H.264
> packetization-mode decoder-match relaxation, and an NVDEC receive opt-in (`LK_NVDEC=1`;
> software receive-decode is the default — NvDecoder's reconfigure SEGVs on simulcast rung
> switches). Its CUDA/NVENC block is **still Linux-arm-only**, so everything in this section
> remains true — but option (c) below is now "extend the in-repo fork", not "create one".
> Also new: the app publishes **simulcast H.264 (rungs top/2 + top/4) — explicitly gated
> NVENC-only** (`simulcast_on` in `crates/streamer/src/call.rs`), so on Windows software
> encode the app cleanly publishes single-layer; adaptive **receive** is portable Rust and
> ports for free.

**Verification #5 is authoritative and overrides any optimistic claim.** The headline
hardware NVENC screenshare will **not** carry to Windows by setting `CUDA_HOME`.

What's true:

- `livekit 0.7.46` (+ the pure-Rust `unpublish_track` patch in `vendor/livekit`) +
  `webrtc-sys 0.3.35` + `libwebrtc 0.3.37` **do build on Windows MSVC**. `webrtc-sys-build`
  downloads the prebuilt **win-x86_64** libwebrtc; the `"windows" =>` build.rs arm links the
  MSVC system libs (`msdmo`, `crypt32`, `d3d11`, `dxgi`, `ws2_32`, …) with `/std:c++20`
  `/EHsc`. The `VideoEncoderBackend::Nvenc` enum still compiles.

What's false:

- The **entire CUDA/NVENC block** (reads `CUDA_HOME`, compiles `src/nvidia/NvEncoder.cpp`,
  `NvEncoderCuda.cpp`, `nvidia_encoder_factory.cpp`, `cuda_context.cpp`, emits
  `-DUSE_NVIDIA_VIDEO_CODEC=1`) is nested **inside the `"linux" =>` arm** of
  `webrtc-sys/build.rs`. The `"windows" =>` arm never reads `CUDA_HOME` and compiles **no**
  `src/nvidia/*.cpp`. So `CUDA_HOME` on Windows is a **no-op**.
- `video_encoder_factory.cpp` adds Nvenc only under `#if defined(USE_NVIDIA_VIDEO_CODEC)`,
  which is undefined on Windows → factory compiled out.
- At runtime `VideoEncoderBackend::list_available()` will **not** contain `Nvenc`, so
  `nvenc_available()` (`lk_publish.rs:159`) returns false and a request for
  `VideoEncoderBackend::Nvenc` (`lk_publish.rs:289`) **silently falls back to software**
  (VP8/VP9/OpenH264). LiveKit's README lists NVENC only for "NVidia discrete GPUs (Linux)."

Options, in order of effort:

- **(a) Accept software encode on Windows for the first build (RECOMMENDED).** VP9/VP8/
  OpenH264 — web/Element-Call interop is fine; higher CPU, not the "hardware NVENC"
  headline. The graceful-degradation code already exists. `nvenc_available()` drives UI, so
  the app stays usable.
- **(b) GStreamer HW encode via a different transport.** Note the real publish hands **raw
  I420** to libwebrtc's `NativeVideoSource`, which re-encodes in software regardless of any
  upstream `nvh264enc`. To get GPU encode you'd switch the publish leg to GStreamer's own
  `livekitwebrtcsink`/WHIP path with a Windows HW encoder (`nvh264enc` / `mfh264enc` /
  `qsv`) instead of the libwebrtc SDK encode. Substantial.
- **(c) Extend the vendored `webrtc-sys` fork (`vendor/webrtc-sys/`).** Add `src/nvidia/*.cpp`
  + `-DUSE_NVIDIA_VIDEO_CODEC` to the `"windows" =>` arm of its `build.rs`, switch cc flags to
  MSVC, supply CUDA headers via `CUDA_HOME`/`CUDA_PATH`, and replace the Linux dlopen shim
  (`add_lazy_load_so`) with `LoadLibrary` of `nvcuda.dll` / `nvEncodeAPI64.dll` /
  `nvcuvid.dll`. The NvCodec sources, factory C++, and build wiring are all already committed
  in-tree — this compiles against the same prebuilt win-x86_64 `libwebrtc` blob, no
  from-source libwebrtc needed. Still real C++/build engineering (LiveKit's commented-out
  VAAPI-on-Windows scaffolding shows they prototyped Windows HW encode but never shipped it),
  but far cheaper than when this doc was first written. Reserve for when hardware encode is a
  hard requirement — and note it also unlocks simulcast publish (NVENC-gated, see the update
  note above).

> **GStreamer `nvcodec` (the gst NVENC path) IS available on Windows** — `nvh264enc`/
> `nvh265enc`/`nvd3d11h264enc` ship in the official 1.28 MSVC bad-plugins and dlopen the
> driver libs at runtime (no CUDA toolkit needed at gst build time). But this only matters
> for the **legacy gst smoke path** in `crates/streamer/src/lib.rs`
> (`force_hardware_h264()`, `livekitwebrtcsink`, `videotestsrc`) — **unused by the real
> E2EE flow** — or for option (b). `nvcodec` elements only **register when an NVIDIA driver
> + supported GPU are present**; on a GPU-less host they silently don't appear, so any
> gst-encode path needs an `mfh264enc`/`x264enc` fallback. On a VM with a passed-through NVIDIA
> GPU `nvh264enc` should appear — confirm with `gst-inspect-1.0 nvh264enc`.

Build config: keep `CUDA_HOME` in `.cargo/config.toml` for Linux; the Windows VM sets its
own only if pursuing option (c). See [`BUILD_WINDOWS.md`](BUILD_WINDOWS.md) for the
`webrtc-sys` build environment.

---

## 8. Plugin & DLL deployment

Full bundle layout, `windeployqt`, and CUDA/mpv DLLs live in
[`PACKAGING.md`](PACKAGING.md). Media-specific essentials:

A portable GStreamer app must ship three things under `%LOCALAPPDATA%\TinHatApp`:

1. **Core DLLs** in `bin\` next to `tinhat.exe`: `libgstreamer-1.0-0.dll`,
   `libgstbase-1.0-0.dll`, `libglib-2.0-0.dll`, `libgobject-2.0-0.dll`, `libgmodule…`,
   `libgio…`, `libffi`, `libintl`, `libpcre2`, `z`/`orc`, plus each plugin's transitive
   codec deps.
2. **Plugin DLLs** in `lib\gstreamer-1.0\`. Trim to only what the pipelines load:
   `gstd3d11.dll`, `gstmediafoundation.dll`, `gstwasapi.dll`, `gstwasapi2.dll`,
   `gstnvcodec.dll` (if pursuing gst-encode), `gstapp.dll`, `gstvideoconvertscale.dll`,
   `gstcoreelements.dll`, `gstaudioconvert`/`gstaudioresample`, `gstrsaudiofx.dll`
   (audiornnoise), `gstsoup.dll` + decoders (uridecodebin), `gstplayback.dll`, and your
   self-built `gstqml6.dll` **only if** you adopt the §6.5 GL path.
3. **`gst-plugin-scanner.exe`** in `libexec\gstreamer-1.0\` (GStreamer forks it to scan
   plugins out-of-process).

Startup env (set in `main()` before any gst call — design these in the `platform` module):

```text
GST_PLUGIN_SYSTEM_PATH = <bundle>\lib\gstreamer-1.0     # prevents scanning system folders
GST_PLUGIN_SCANNER     = <bundle>\libexec\gstreamer-1.0\gst-plugin-scanner.exe
GST_REGISTRY           = %LOCALAPPDATA%\TinHatApp\cache\gst-registry.bin   # writable per-user
```

Caveats:

- `gst-plugin-scanner` has had **Windows path-resolution regressions** — it must find the
  core DLLs to scan, so keep core DLLs in `bin\` **and** on PATH, **or** call
  `gst_registry_fork_set_enabled(false)` to scan in-process.
- A single missing transitive codec DLL makes a plugin **silently** fail to load (no error
  until the element is requested). Use `dumpbin /dependents` (or Dependencies.exe) on each
  plugin DLL to find its imports.
- A **stale registry cache** after a self-update can hide newly added plugins — **clear/bump
  the cache on version change** (the self-updater in [`UPDATE_PATH.md`](UPDATE_PATH.md) must
  also replace plugin DLLs and invalidate this cache).

Validate on a clean VM: `gst-inspect-1.0 <element>` against the bundle path with
`GST_DEBUG=3` to confirm every needed element loads with no missing-DLL errors. Quick
checklist: `d3d11screencapturesrc`, `mediafoundationvideosrc`, `wasapisrc`, `wasapisink`,
`appsink`, `appsrc`, `videoconvertscale`, `audiornnoise`, `uridecodebin`, and (if used)
`nvh264enc`.

---

## 9. Element-by-element substitution reference

| Linux element / construct | File (Linux) | Windows replacement | Notes |
|---|---|---|---|
| `pipewiresrc fd= path=` (screen) | `capture.rs`, `lk_publish.rs` | `d3d11screencapturesrc monitor-index=`/`window-handle=` | no fd/node; needs native picker; output = D3D11Memory |
| portal `ScreenCast` session | `capture.rs` | `ScreenSource` trait + WGC/`d3d11` handle | no session lifecycle to hold |
| `video/x-raw(ANY)` + `videoconvert` readback | `lk_publish.rs` | `video/x-raw(ANY)` + `d3d11download` (+`d3d11convert`) | preserve the (ANY) anti-hitch trick vs D3D11Memory |
| `do-timestamp` / `keepalive-time` | `lk_publish.rs` | (drop) | pipewiresrc-specific |
| `v4l2src device=/dev/videoN` | `camera.rs` | `mediafoundationvideosrc device-path=` / `ksvideosrc` | MF symbolic-link id; jpegdec branch stays |
| `DeviceMonitor` Video/Source `device.path`/`api.v4l2.path` | `camera.rs` | `DeviceMonitor` Video/Source, MF `device.path` | mfdeviceprovider |
| `pipewiresrc target-object=` (mic) | `call.rs` | `wasapisrc device=` / `wasapi2src` | bare = default |
| `pipewiresink target-object=` (playback) | `call.rs` | `wasapisink device=` / `wasapi2sink` | NULL-cycle re-point works |
| `pipewiresrc "<sink>.monitor"` (loopback) | `call.rs` | `wasapisrc loopback=true device=<render-id>` | **wasapi only** (wasapi2 lacks loopback); whole-endpoint only |
| `DeviceMonitor` Audio/Sink|Source `node.name` | `call.rs` | `DeviceMonitor`, WASAPI device id | node.name absent on Windows |
| `pactl get-default-source/sink` | `call.rs` | `IMMDeviceEnumerator::GetDefaultAudioEndpoint` or "" = default | Core Audio |
| `audiornnoise` | `call.rs` | `audiornnoise` | same element; bundle gstrsaudiofx.dll |
| `uridecodebin` | `call.rs` | `uridecodebin` | bundle souphttpsrc + decoders |
| `audioconvert`/`audioresample`/`audiobuffersplit`/`audiorate`/`queue`/`appsink`/`appsrc`/`volume` | `call.rs` | identical | fully portable |
| `qml6glsink`+`glupload`+`glcolorconvert` | `camera.rs`, `video_bridge.cpp` | **appsink → QVideoSink → QML VideoOutput** | primary; qml6glsink = gated experiment |
| `tinhat_pin_gstgl_for_qpa` wayland/xcb | `video_bridge.cpp` | windows branch `GST_GL_PLATFORM=wgl` (only if GL path) | else N/A under appsink path |
| libwebrtc NVENC factory (CUDA) | `webrtc-sys` | **software VP9/OpenH264** | not built on Win; see §7 |
| gst `nvh264enc`/`livekitwebrtcsink` (smoke) | `lib.rs` | available but unused by E2EE flow | needs NVIDIA driver to register |
| `ashpd` (screencast) | `crates/streamer/Cargo.toml` | (drop — `cfg(target_os="linux")`) | no D-Bus portal |

---

## 10. Open questions / risks

1. **§6 video sink is the #1 risk.** Prototype the **appsink → QVideoSink** path EARLY on
   the actual VM (with its passed-through GPU). It is the recommended primary path precisely because
   `qml6glsink`-on-Windows is unproven (D3D11-default RHI, no upstream plugin, Intel-driver +
   green-flash + software-GL-fallback hazards). Confirm multi-tile grid + self-view +
   bubble-morph all render before committing.
2. **OpenGL-forcing conflict.** The mpv watch-party (`MpvAbstractItem`) is OpenGL-only and
   needs `setGraphicsApi(OpenGL)`; the recommended call-tile path deliberately avoids forcing
   OpenGL (to keep the D3D11 UI stable). If both must coexist, decide: force OpenGL globally
   (and validate the call tiles still work) vs. give mpv its own OpenGL window vs. defer
   watch-party on Windows. See [`BUILD_WINDOWS.md`](BUILD_WINDOWS.md) §mpv.
3. **NVENC parity.** First Windows build ships software encode (§7 option a). Confirm with
   the user that this is acceptable for v1; option (c) is a separate, large effort.
4. **Soundboard/virtual-sink redesign** (§5) is a feature decision, not a port. Pick mixer
   vs. WASAPI-loopback vs. RODECaster-hardware and document it.
5. **`d3d11screencapturesrc` Win11 24H2 window-capture stall** (§1.5) — prefer monitor
   capture or `capture-api=wgc`; test on the VM's exact build.
6. **WASAPI 10 ms feeder** (§3.1) — re-verify the `audiobuffersplit` re-blocking feeds
   libwebrtc exactly 480 samples given WASAPI's quantum; the mic-crackle fix depends on it.
7. **Re-validate the r113 camera-squish fix** (§2.2) under whatever §6 sink you choose,
   using `TINHAT_CAM_GLTEST` + `TINHAT_CAM_FORCE`.
8. **gst-plugin-scanner / stale registry** (§8) — wire cache invalidation into the
   self-updater or newly bundled plugins won't be seen after an update.
