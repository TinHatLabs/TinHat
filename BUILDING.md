# Building Tin Hat

Rust (1.98+) + CXX-Qt 0.8 + Qt 6.11 / KDE Frameworks 6 (Kirigami) + GStreamer 1.26+.

## Linux (native)

Arch package names, for reference: qt6-base qt6-declarative qt6-webengine qt6-multimedia qt6-5compat qt6-svg
qt6-imageformats kirigami kconfig kcolorscheme qqc2-desktop-style breeze-icons layer-shell-qt gstreamer
gst-plugins-{base,good,bad} gst-libav gst-plugin-rsaudiofx gst-plugin-qml6 gst-plugin-pipewire pipewire
libpulse mpvqt yt-dlp, plus `cuda` (headers only; NVENC is dlopen'd at runtime) and `libva` (VA-API encode on
AMD/Intel is dlopen'd too).

```
export CUDA_HOME=/opt/cuda                      # or wherever cuda.h lives
export LK_CUSTOM_WEBRTC=/path/to/libwebrtc/linux-x64-release   # the patched archive (see vendor/webrtc-sys/libwebrtc-patches/README.md)
cargo build --release -p tinhat-app        # -> target/release/tinhat
```

The binary must carry both ship stamps (`tinhat-av1-e2ee-v2`, `tinhat-dav1d-fp-v1`) — `grep` them; a build
without them linked the upstream libwebrtc and encrypted AV1 will not decode.

## Release builds

A release binary shouldn't carry the build machine's paths (Rust panic locations, C/C++ `__FILE__`). Run
`scripts/release-paths.sh` (Linux) or `windows\release-paths.ps1` (Windows) in the checkout, then build with
`--config .cargo/release-paths.toml` (git-ignored: it maps the cargo home, the checkout, the target dir and
`LK_CUSTOM_WEBRTC` to `/cargo`, `/tinhat`, `/target`, `/webrtc`). MSVC has no prefix map for C/C++, so on Windows also
build from a neutral folder with a neutral `CARGO_HOME` (e.g. `C:\tinhat\src`, `C:\cargo`). Don't use the `RUSTFLAGS`
variable for this: it replaces the repository's own rustflags.

## Flatpak

App id `dev.tinhat.TinHat`. The manifest builds everything offline from vendored sources; it isn't in this
repository yet.

## Windows

`windows/BUILD_WINDOWS.md` + `windows/dev-env.ps1`; `windows/package.ps1 -SkipBuild` makes the zip and, with
Inno Setup (`ISCC /DAppVer=<version> windows\installer.iss`), the installer.

## Gates

Before a commit: `cargo test --release`, `bash scripts/qml-lint.sh`, `python3 scripts/qml-boundary.py`,
`python3 scripts/theme-lint.py`, `python3 scripts/actions-coverage.py`, `node --test tools/seeds-test.mjs
tools/invite-link-test.mjs`. The behaviour and pixel gates run on the maintainers' test machines and are not part
of this repository.
