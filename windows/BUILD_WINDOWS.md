# BUILD_WINDOWS.md — Windows toolchain setup + first build

Part of the Windows port handoff set. Start at [`../HANDOFF_WINDOWS.md`](../HANDOFF_WINDOWS.md).
Read alongside:
- [`PLATFORM_AUDIT.md`](PLATFORM_AUDIT.md) — the `cfg(target_os)` module layout this doc's `build.rs` changes plug into, and the **theming decision** (drop KColorScheme).
- [`MEDIA_PIPELINE.md`](MEDIA_PIPELINE.md) — the GStreamer element port, **how to build `qml6glsink` yourself**, and the NVENC-on-Windows reality.
- [`PACKAGING.md`](PACKAGING.md) — `windeployqt` + DLL/plugin bundling into the portable zip (this doc gets you a *running dev build*; that doc makes it *shippable*).
- §8 below — getting the source (this repository, incl. `vendor/livekit`).

**Scope of THIS doc:** get the source compiling and linking into a runnable `tinhat.exe` on a Windows 11 machine (or VM). Toolchain install, env, the exact `cargo build`, and the `build.rs` edits required. **Design/instructions only** — do not modify Linux source/build files until you are on the Windows VM doing the real `cfg(windows)` work.

---

## 0. The one decision that governs everything: MSVC, not MinGW

Build the **entire** stack with the **MSVC** toolchain (`x86_64-pc-windows-msvc`). This is non-negotiable because the load-bearing pieces are all MSVC-native:

- Rust's default Windows toolchain links the **release** MSVC CRT (`ucrt`/`vcruntime`).
  - **CRT flavor (learned the hard way on the VM):** the prebuilt LiveKit `libwebrtc` is
    **static-CRT (`/MT`)** — Chromium's `is_component_build=false` default — so Rust's default
    dynamic-CRT (`/MD`) link dies in `LNK2038 RuntimeLibrary` mismatches. Fix: build with
    `-C target-feature=+crt-static`, already committed on the `windows-port` branch as a
    `[target.x86_64-pc-windows-msvc]` rustflags section in `.cargo/config.toml` (Windows-only;
    Linux untouched). Same approach as upstream livekit rust-sdks.
- `webrtc-sys` (via `livekit`) downloads a **prebuilt MSVC** `libwebrtc` and its `build.rs` `"windows"` arm compiles the cxx bridge with `/std:c++20 /EHsc`.
- CXX-Qt's own CI builds exactly this shape — `windows-2022`, `cl`, target `x86_64-pc-windows-msvc`, against Qt 6.7.3 / 6.10.1 (verification #1).
- `aws-lc-rs`/`ring` (TLS, pulled by matrix-sdk/livekit) build cleanly under MSVC with NASM + CMake.

> **Do NOT mix MinGW and MSVC artifacts in one process.** A MinGW-built Qt plugin (or `qml6glsink`, or `MpvQt`) loaded into an MSVC app will fail to load or crash. Everything that ends up in-process — Qt, the Qt Quick Controls style, `qml6glsink`, Kirigami's C++ plugin, KF6 — must be the **same Qt minor version, same MSVC ABI**. This single rule is the source of most of the open risks at the bottom.

The unavoidable wart: **mpv / mpvqt have no MSVC build** (upstream mpv is GCC/Clang only; KDE Craft builds them `GCCLike`). That is the only piece that cannot join the MSVC party. The recommendation (below, and in `MEDIA_PIPELINE.md`) is to **`cfg`-gate the watch-party mpv surface OUT of the first Windows build** and revisit it later — it is not on the core chat/call/screenshare path.

---

## 1. Component checklist

| Component | Version / variant | Why | Notes |
|---|---|---|---|
| Visual Studio 2022 **Build Tools** | "Desktop development with C++" + Windows 11 SDK | MSVC `cl`/`link`, the C++ shims, `cc`, libwebrtc bridge | Build Tools (no full IDE) is enough |
| Rustup | stable-`x86_64-pc-windows-msvc` | the workspace | `rustup default stable-msvc` |
| NASM | latest | `aws-lc-rs` / `ring` asm | must be on `PATH` |
| CMake | ≥ 3.25 | `aws-lc-rs` / `ring` / bundled-sqlite | on `PATH` |
| Strawberry Perl | latest | occasionally needed by `aws-lc-rs` | install if TLS crate build complains |
| Git for Windows | latest | source clone + `build.rs` git-hash | `git` on `PATH` |
| **Qt 6** | **6.8 LTS** `msvc2022_64`, **Release** | the whole GUI | pin this version everywhere — see §4 |
| GStreamer | **1.28.x MSVC**, devel + runtime | media pipeline | official `.exe`, `/TYPE=devel` for build |
| pkgconf (Windows) | latest | `build.rs` discovers gstreamer/mpv `.pc` | not present by default |
| CUDA Toolkit | optional | **NO-OP for NVENC on stock build** | see §6 — install only if you attempt the webrtc-sys fork |
| KDE Craft | MSVC profile | Kirigami / KF6 / (mpvqt) | the friction point — see §7 |

Verify the basics in a fresh **"x64 Native Tools Command Prompt for VS 2022"** (this shell puts `cl`, `link`, and the SDK in `PATH` — run *all* cargo builds from here):

```bat
cl                 :: should print the MSVC version banner
rustup show        :: default host should be x86_64-pc-windows-msvc
nasm --version
cmake --version
git --version
```

---

## 2. Rust toolchain

```bat
:: from https://rustup.rs (rustup-init.exe)
rustup default stable-msvc
rustup target add x86_64-pc-windows-msvc   :: usually already the host target
```

No `rust-toolchain.toml` exists in the repo, so stable is fine. Keep `cargo` runs inside the **x64 Native Tools** prompt so `cl`/`link`/`nasm` resolve.

---

## 3. Native crypto build deps (NASM / CMake / Perl)

`matrix-sdk` pulls `aws-lc-rs`; `livekit` pulls `ring`. Both compile C/asm at build time and are the classic Windows tripwire. Before the first `cargo build`:

1. Install **NASM**, add its dir to `PATH`.
2. Install **CMake**, add to `PATH`.
3. If `aws-lc-rs` errors on a missing Perl, install **Strawberry Perl** and add to `PATH`.

These produce no source changes — they are pure build prerequisites. If TLS crates fail, this trio is almost always why.

> The app installs a single rustls provider in `main()` because both `aws-lc-rs` (matrix-sdk) and `ring` (livekit) are enabled — that logic is platform-agnostic and needs no change.

---

## 4. Qt 6 — version, modules, and how CXX-Qt actually finds it

### 4.1 Install

Use the **official Qt Online Installer**. Pick **Qt 6.8 (LTS)** → **MSVC 2022 64-bit**. Select these components/modules (mirrors the Linux `qt6-*` deps):

- Qt Quick / Qt Declarative (Core, Gui, Qml, Quick — base)
- **Qt Multimedia** (camera/audio surfaces; the appsink→`QVideoSink` render fallback)
- **Qt 5 Compatibility Module** (`Qt5Compat` — Kirigami needs it)
- **Qt Image Formats / SVG** (icons)
- **Qt WebEngine** (inline iframe embeds; bundles `QtWebEngineProcess.exe` + Chromium resources — see PACKAGING)

Result path (example): `C:\Qt\6.8.0\msvc2022_64\`.

> **Use a Release Qt build, even for a debug Rust build.** Rust always links the *release* MSVC CRT, so debug Qt libs (`*d.dll`) mismatch the CRT and fail to link/load. Simplest: build the app `--release` (§10). (CXX-Qt auto-defines `QT_NO_DEBUG` only for dynamic QML plugins, not for your binary — so don't rely on it; just use release Qt.)

### 4.2 CRITICAL — CXX-Qt locates Qt via **qmake**, NOT `CMAKE_PREFIX_PATH`/`Qt6_DIR`

This corrects a common misconception (verification #1). This project is a **pure-cargo** build: `build.rs` + `CxxQtBuilder` with **no `CMakeLists.txt`**. In that mode `cxx-qt-build` delegates to `qt-build-utils`, whose `QtBuild::new()` finds Qt **only** through:

1. the **`QMAKE`** environment variable, or
2. `qmake` on `PATH` (multiplexed by `QT_VERSION_MAJOR`).

It does **not** read `CMAKE_PREFIX_PATH` or `Qt6_DIR` — those are CMake variables, irrelevant to the cargo path. Setting them alone yields a "QtCore not found"-class failure (cf. cxx-qt issues #885, #625).

**So, before building, set:**

```bat
set QMAKE=C:\Qt\6.8.0\msvc2022_64\bin\qmake.exe
```

(or prepend `C:\Qt\6.8.0\msvc2022_64\bin` to `PATH`). That one variable drives both cxx-qt's QML-module link and — once you wire §9 — the C++ shim's Qt include/lib discovery.

---

## 5. GStreamer (MSVC) + pkgconf

### 5.1 Install

GStreamer **1.28.x** ships as a single MSVC installer per arch:
`gstreamer-1.0-msvc-x86_64-1.28.x.exe` from <https://gstreamer.freedesktop.org/download/>.

Install **both** what you need: choose **Complete**, and you want the **devel** content (headers, `.lib` import libs, **`.pc` pkg-config files**, `gst-inspect`/`gst-launch`). The installer sets `GSTREAMER_1_0_ROOT_MSVC_X86_64` (e.g. `C:\gstreamer\1.0\msvc_x86_64\`). Put `%GSTREAMER_1_0_ROOT_MSVC_X86_64%\bin` on `PATH`.

The `gstreamer` Rust bindings in the workspace are `0.23` and are forward-compatible with the 1.28 runtime.

### 5.2 pkgconf + `PKG_CONFIG_PATH`

MSVC has no `pkg-config`. The GStreamer MSVC devel install **does ship `.pc` files**, so the existing `pkg_config` probe for `gstreamer-1.0` keeps working once you provide a `pkgconf.exe` and point it at them:

- Install **pkgconf** (e.g. `winget install pkgconf`, or the chocolatey/standalone build), ensure `pkgconf.exe` is on `PATH` (the `pkg-config` crate also honors a `PKG_CONFIG` env var pointing at the binary).
- Set:

```bat
set PKG_CONFIG_PATH=%GSTREAMER_1_0_ROOT_MSVC_X86_64%\lib\pkgconfig
```

### 5.3 The Qt sink is NOT in the GStreamer binaries

Confirmed (verification #2/#5): **upstream GStreamer ships no `qml6` plugin on any platform** — `qml6glsink`/`gstqml6.dll` is absent from the Windows installer. The app renders remote/camera tiles through `qml6glsink`, so you must build it yourself against **the same Qt 6.8 MSVC** the app links, or adopt the `appsink → QVideoSink` fallback. **That whole decision lives in [`MEDIA_PIPELINE.md`](MEDIA_PIPELINE.md)** — do not try to solve it here. For the *first build* you can compile the shim and let `qml6glsink` be resolved at runtime (it is `gst_element_factory_make`'d, not linked), so a missing plugin fails the video path, not the build.

### 5.4 mpv `.pc`

The `build.rs` also `pkg_config`-probes `mpv` and links `MpvQt`. On Windows neither is available MSVC-native (mpv is MinGW-only). **Recommendation: `cfg`-gate the mpv/`MpvQt` discovery out of the Windows build (§9)** and disable the watch-party mpv surface for the first build. libmpv itself ports (shinchiro builds), but `MpvQt` would need a Craft/source build against the app's Qt — defer per `MEDIA_PIPELINE.md`.

---

## 6. CUDA toolkit + `CUDA_HOME` — read this before you waste a day

`/.cargo/config.toml` sets `CUDA_HOME = "/opt/cuda"` to enable the libwebrtc **NVENC** factory in `webrtc-sys`.

**On Windows this is a no-op for NVENC** (verification #7, *refuted*). Verified against the pinned crates: in `webrtc-sys 0.3.35`'s `build.rs`, the entire CUDA/NVENC block (reads `CUDA_HOME`, compiles `src/nvidia/*.cpp`, emits `-DUSE_NVIDIA_VIDEO_CODEC=1`) lives **exclusively inside the `"linux" =>` match arm**. The `"windows" =>` arm only links MSVC system libs and builds the cxx bridge — it never reads `CUDA_HOME` and never compiles the NVENC factory. *(2026-07-05: `webrtc-sys` now resolves to the vendored in-repo fork `vendor/webrtc-sys/` via `[patch.crates-io]` — same gating, same conclusion; see HANDOFF §0.)* So:

- The build **succeeds** on Windows (it downloads the prebuilt MSVC `libwebrtc`).
- `VideoEncoderBackend::Nvenc` **still compiles** (the enum is cross-platform).
- But at runtime `list_available()` will **not** contain `Nvenc`; `nvenc_available()` (`crates/streamer/src/lk_publish.rs`) returns false; requesting `Nvenc` **silently falls back to software** (VP9/VP8/OpenH264).

**Implication for the first build:** do **not** install CUDA expecting hardware NVENC. The headline GPU screenshare will run on **software encode** on Windows out of the box. That is acceptable for parity (web/Element-Call interop is fine; just higher CPU). Getting real hardware encode means either a GStreamer-side encoder path (`nvh264enc`/`mfh264enc`) or **extending the vendored fork's `"windows" =>` arm** (`vendor/webrtc-sys/` — the NvCodec sources are already in-tree; see HANDOFF §0) — bounded but real C++/build work, out of scope for the first build. Full analysis + options in [`MEDIA_PIPELINE.md`](MEDIA_PIPELINE.md).

> On the Windows VM, leave the Linux `CUDA_HOME=/opt/cuda` line alone (it's harmless/ignored on Windows). Only set a Windows `CUDA_PATH` if/when you attempt the fork.

---

## 7. KDE Craft — Kirigami / KF6 (and the theming simplification)

The UI shell is **Kirigami**; on Linux full-app theming also uses **KF6 KColorScheme/KColorSchemeManager** + **qqc2-desktop-style**. On Windows:

### 7.1 What you must build via Craft
- **Kirigami** (the UI components) — required, in-process C++/QML plugin → **must be built MSVC against Qt 6.8** to match the app ABI. Craft can use an MSVC profile; pin Craft's Qt to the same 6.8.

### 7.2 What to DROP (the theming decision — verification #3)
KColorScheme and qqc2-desktop-style are a **matched pair**. KColorScheme generates `.colors` and activates them through `KColorSchemeManager`, which **only** styles chrome because `qqc2-desktop-style` reads it. You cannot keep `qqc2-desktop-style` and drop KColorScheme — the chrome would then be painted by the Windows-native QStyle palette (the exact bug `colorscheme.rs` was written to fix returns).

**Recommended Windows path (lighter, smaller zip):** drop **both**, and force a `Kirigami.Theme`-respecting Qt Quick Controls style:

- Force the style **before** the QML engine loads — `QQuickStyle::setStyle("org.kde.breeze")` (qqc2-breeze-style, pure-QML, palette/`Kirigami.Theme`-driven, Craft-buildable) **or** Qt's built-in **`Basic`** style (fully palette-driven, zero extra deps).
- `qml/Main.qml` already sets both the QQC2 `palette` and `Kirigami.Theme.*` (`inherit:false`), so with a palette-respecting style the whole UI (incl. chrome) recolors from the portable `Pal` table.

This lets you **drop `qqc2-desktop-style`, `kconfigwidgets`, and `kcolorscheme` from the Windows dependency tree entirely** — and is why §9 removes the KF6 includes/links from `build.rs`. (Avoid the Material style as the bundled style unless you also set `Material.accent/background/theme` — it doesn't take all colors from the palette.) The full rationale and the `colorscheme.rs`/`video_bridge.cpp` stub plan live in [`PLATFORM_AUDIT.md`](PLATFORM_AUDIT.md).

### 7.3 breeze-icons
Windows/Qt has **no system icon theme** — the UI will be full of blank squares unless you bundle **breeze-icons** and call `QIcon::setThemeSearchPaths` + `setThemeName("breeze")` at startup. Build/fetch the breeze icon set (Craft or source); bundling is a PACKAGING concern but note it now so the dev build's icons aren't mysteriously empty.

### 7.4 mpvqt — defer
`MpvQt` (watch-party) has no MSVC build. Defer per §5.4 / `MEDIA_PIPELINE.md`.

---

## 8. Get the source

Clone this repository (`git clone https://github.com/TinHatLabs/TinHat`) and build its default branch or a
release tag.

- The clone **must include `vendor/livekit/` and `vendor/webrtc-sys/`** (committed vendored forks). The workspace `[patch.crates-io]` resolves both: the `livekit` patch (`unpublish_track`) is pure portable Rust and compiles on Windows unchanged; the `webrtc-sys` fork (NVENC pinning, packetization-mode relaxation, `LK_NVDEC` opt-in) builds on Windows via its `"windows" =>` arm — its NVENC block stays Linux-only (§6).

Confirm `git rev-parse --short HEAD` works in the checkout — `build.rs` bakes it into `TINHAT_BUILD_HASH` (falls back to "dev" if git is absent; works on Windows unchanged).

---

## 9. `build.rs` changes (the only build-file edit, done on the VM)

Today `crates/app/build.rs` is Linux-only after the `CxxQtBuilder` block: it `pkg_config`-probes Qt + gstreamer + mpv, hardcodes `/usr/include/KF6...`, and links `KF6ColorScheme`/`KF6ConfigCore`/`KF6ConfigGui`/`MpvQt` with `rustc-link-search=/usr/lib`. All of that must become `cfg(target_os)`-gated. The `CxxQtBuilder::new_qml_module(...).build()` portion and the git-hash block stay **as-is** (cross-platform).

**What stays unchanged:**
- The whole `CxxQtBuilder` QML-module registration + `.file(...)` list + `.build()`.
- The git-hash → `TINHAT_BUILD_HASH` block and `rerun-if-changed=../../.git/HEAD`.
- `cc::Build` compiling `video_bridge.cpp` + `webengine_bridge.cpp` (these compile under MSVC; `-fPIC` is already `flag_if_supported` so it's a no-op on MSVC; `.std("c++17")` maps to `/std:c++17`).

**What must be gated.** Keep the existing block under `#[cfg(target_os = "linux")]` and add a Windows sibling. Sketch:

```rust
// ... CxxQtBuilder ... .build();  // unchanged

let mut build = cc::Build::new();
build.cpp(true)
    .file("src/video_bridge.cpp")
    .file("src/webengine_bridge.cpp")
    .std("c++17")
    .flag_if_supported("-fPIC");   // no-op on MSVC

#[cfg(target_os = "linux")]
{
    // EXISTING behaviour, verbatim:
    for pkg in ["Qt6Quick","Qt6Qml","Qt6WebEngineQuick","gstreamer-1.0","mpv"] {
        let lib = pkg_config::Config::new().probe(pkg)
            .unwrap_or_else(|e| panic!("pkg-config {pkg} failed: {e}"));
        for inc in &lib.include_paths { build.include(inc); }
    }
    for inc in ["/usr/include/KF6","/usr/include/KF6/KColorScheme",
                "/usr/include/KF6/KConfigCore","/usr/include/KF6/KConfigGui"] {
        build.include(inc);
    }
    for inc in ["/usr/include","/usr/include/MpvQt"] { build.include(inc); }
    build.compile("tinhatvideobridge");
    println!("cargo:rustc-link-lib=KF6ColorScheme");
    println!("cargo:rustc-link-lib=KF6ConfigCore");
    println!("cargo:rustc-link-lib=KF6ConfigGui");
    println!("cargo:rustc-link-lib=MpvQt");
    println!("cargo:rustc-link-search=native=/usr/lib");
}

#[cfg(target_os = "windows")]
{
    // 1) GStreamer via pkgconf (.pc ships in the MSVC devel install; PKG_CONFIG_PATH set in env)
    let gst = pkg_config::Config::new().probe("gstreamer-1.0")
        .expect("gstreamer-1.0 not found: install GStreamer 1.28 MSVC *devel* and set PKG_CONFIG_PATH to its lib\\pkgconfig");
    for inc in &gst.include_paths { build.include(inc); }

    // 2) Qt include/lib paths from qmake (NO pkg-config for Qt on Windows MSVC).
    //    Query qmake for headers + libs and add Quick/Qml/WebEngineQuick subdirs.
    let qmake = std::env::var("QMAKE")
        .expect("set QMAKE=...\\msvc2022_64\\bin\\qmake.exe");
    let q = |var: &str| {
        let out = std::process::Command::new(&qmake).args(["-query", var]).output().unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    let qt_headers = q("QT_INSTALL_HEADERS"); // e.g. C:/Qt/6.8.0/msvc2022_64/include
    let qt_libs    = q("QT_INSTALL_LIBS");    // e.g. C:/Qt/6.8.0/msvc2022_64/lib
    for m in ["", "QtQuick", "QtQml", "QtWebEngineQuick", "QtGui", "QtCore"] {
        build.include(format!("{qt_headers}/{m}"));
    }
    build.include(&qt_headers);
    build.flag_if_supported("/EHsc");
    build.compile("tinhatvideobridge");

    // 3) Link Qt libs the shim references (cxx-qt already links Core/Gui/Qml).
    println!("cargo:rustc-link-search=native={qt_libs}");
    for l in ["Qt6Quick","Qt6WebEngineQuick","Qt6WebEngineCore"] {
        println!("cargo:rustc-link-lib={l}");
    }
    // 4) NO KF6 / NO MpvQt on Windows:
    //    - KColorScheme theming is dropped (§7.2) -> the KF6 includes/links are gone.
    //    - video_bridge.cpp's tinhat_apply_color_scheme must be #ifdef'd to a QPalette stub
    //      on Windows (keep the extern "C" symbol stable so video.rs FFI is unchanged).
    //    - mpvqt watch-party deferred (§5.4) -> #ifdef out the MpvQt include + tinhat_mpv_register.
    //    GStreamer link libs come from pkg-config metadata above; mpv link dropped.
}
```

Companion source edits the Windows build needs (detailed in [`PLATFORM_AUDIT.md`](PLATFORM_AUDIT.md), listed here so the build links):
- **`video_bridge.cpp`**: `#ifdef`-out `#include <KColorSchemeManager>` + the `tinhat_apply_color_scheme` KF6 body → QPalette stub (keep the `extern "C"` symbol). `#ifdef`-out `#include <MpvQt/MpvAbstractItem>` + `tinhat_mpv_register` if deferring mpv. The `qml6glsink`/GstGL bridge stays; `tinhat_pin_gstgl_for_qpa` needs a `platformName()=="windows"` branch (`GST_GL_PLATFORM=wgl`/`GST_GL_API=opengl`) — see MEDIA_PIPELINE.
- **`Cargo.toml`**: gate `libc` under `[target.'cfg(unix)'.dependencies]`, `zbus` under `[target.'cfg(target_os="linux")'.dependencies]`, add `[target.'cfg(windows)'.dependencies]` (`windows`/`windows-sys`, toast crate) + common `dirs`. See PLATFORM_AUDIT.
- **`crates/streamer/Cargo.toml`**: gate `ashpd` under `cfg(unix)`/`cfg(target_os="linux")` (no D-Bus portal on Windows).

---

## 10. The environment + the build command

Open **"x64 Native Tools Command Prompt for VS 2022"** and set:

```bat
:: Qt (drives BOTH cxx-qt-build AND the build.rs qmake queries)
set QMAKE=C:\Qt\6.8.0\msvc2022_64\bin\qmake.exe
set PATH=C:\Qt\6.8.0\msvc2022_64\bin;%PATH%

:: GStreamer (installer set GSTREAMER_1_0_ROOT_MSVC_X86_64; ensure bin on PATH)
set PATH=%GSTREAMER_1_0_ROOT_MSVC_X86_64%\bin;%PATH%
set PKG_CONFIG_PATH=%GSTREAMER_1_0_ROOT_MSVC_X86_64%\lib\pkgconfig

:: pkgconf must be reachable (pkg-config crate). If not on PATH:
:: set PKG_CONFIG=C:\path\to\pkgconf.exe

:: NASM / CMake / Git assumed already on PATH (see §1)

:: Optional verbose link if QML object files misbehave:
:: set RUST_LOG=cxx_qt_build=debug
```

Then build **release** (matches the release MSVC CRT and release Qt libs — see §4.1):

```bat
cargo build --release -p tinhat-app
```

Output: `target\release\tinhat.exe` (bin name is `tinhat`, per `[[bin]]`).

> Why release: a debug Rust build still links the release CRT, which mismatches debug Qt (`*d.dll`). Building `--release` sidesteps the whole debug-CRT/debug-Qt mismatch. If you genuinely need a debug Rust build, you must point Qt at a **release** Qt install anyway.

The `.exe` will **not** run yet from `target\release\` — Qt6 DLLs, the Qt QML plugin tree, GStreamer plugin DLLs + `gst-plugin-scanner.exe`, breeze-icons, and (if used) `qml6glsink` are not gathered by cargo. To make it run/launch, follow [`PACKAGING.md`](PACKAGING.md) (`windeployqt` against the exe + the GStreamer/plugin bundle + `GST_PLUGIN_PATH`/`GST_PLUGIN_SCANNER`/`GST_REGISTRY`). For a quick dev smoke without packaging, prepend the Qt `bin` and GStreamer `bin` to `PATH` and set `set QT_QUICK_CONTROLS_STYLE=Basic` so controls render.

---

## 11. First-run smoke (dev, pre-packaging)

1. `gst-inspect-1.0.exe d3d11screencapturesrc` / `mfvideosrc` / `wasapisrc` / `nvh264enc` / `appsink` — confirm the elements the port will use exist (MEDIA_PIPELINE maps them). `nvh264enc` only registers if the NVIDIA driver is present (a GPU passed through to a VM is fine).
2. Launch `tinhat.exe` with Qt+GStreamer `bin` on `PATH`. The single-instance lock and stderr→logfile paths are Linux-coded today; until the `platform::` module lands they may need `TINHAT_NO_SINGLE_INSTANCE=1`. Logs go to the app's data folder on Windows (`log_dir()` in `crates/app/src/platform.rs`).
3. Login + chat verify the cross-platform core (matrix-sdk, TLS, sqlite store) compiled and links — this proves §3 worked even before media is wired.

---

## Open questions / risks

1. **Qt ABI unification (highest risk).** Kirigami's C++ plugin, `qml6glsink` (`gstqml6.dll`), the chosen QQC2 style, and any KF6 you keep **must** all be MSVC, same Qt 6.8 minor. Any MinGW artifact (notably from a default Craft profile) breaks load. Pin one Qt version across the Qt installer, Craft, and the self-built `qml6` plugin. Validate Kirigami loads in-process before going deep.
2. **`qml6glsink` does not exist in the GStreamer Windows binaries** and requires forcing the OpenGL RHI (Qt6 defaults to D3D11 on Windows), with documented driver/multi-tile reliability caveats. The recommended primary path may instead be `appsink → QVideoFrame → QVideoSink → QML VideoOutput` (backend-agnostic, no custom plugin). **Decision deferred to [`MEDIA_PIPELINE.md`](MEDIA_PIPELINE.md)** — don't block the toolchain build on it.
3. **NVENC is not available on Windows with `webrtc-sys`** (verification #7; still true of the vendored fork — its NVENC block is Linux-arm-only). First build = software encode. Hardware encode needs a GStreamer-encoder path or extending `vendor/webrtc-sys/`'s Windows arm (bounded, see HANDOFF §0). `CUDA_HOME` on Windows does nothing — don't be misled.
4. **mpv / mpvqt have no MSVC build.** Watch-party mpv surface is `cfg`-gated out of the first build; revisit (Craft GCCLike build vs Qt Multimedia/WebEngine fallback) per MEDIA_PIPELINE.
5. **pkgconf + `.pc` availability for mpv.** GStreamer ships `.pc`; mpv does not on MSVC. The `build.rs` Windows branch drops the `mpv` probe accordingly — re-add only when libmpv/MpvQt are built for the VM.
6. **Craft bootstrap friction.** Getting Kirigami/KF6 out of Craft with an MSVC profile pinned to Qt 6.8 is the single biggest setup-time unknown after the Qt ABI rule. Budget time; it's the gate for a themed, icon'd UI.
