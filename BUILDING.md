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

The binary must carry the four ship stamps (`tinhat-av1-e2ee-v2`, `tinhat-dav1d-fp-v1`, `tinhat-vp9-kf-iv-v1`, `tinhat-iv-v2`) — `grep` them;
a build without them linked the upstream (or an older) libwebrtc: encrypted AV1 will not decode, and encrypted VP9
shares can wait or never start at the SFU.

## Release builds

Every release train builds with `TINHAT_RELEASE_BUILD=1` and runs `scripts/release-binary-check.sh <binary>` on the
result. It requires the four libwebrtc stamps and refuses the pre-rename ones and the developer self-test hooks
`TINHAT_LKTEST` / `TINHAT_CALLTEST` / `TINHAT_CAMTEST`. Those hooks seal with a throwaway test key and exist only outside
a release: `build.rs` sets `cfg(tinhat_env_selftests)` unless `TINHAT_RELEASE_BUILD=1`.

A release binary shouldn't carry the build machine's paths (Rust panic locations, C/C++ `__FILE__`). Run
`scripts/release-paths.sh` (Linux) or `windows\release-paths.ps1` (Windows) in the checkout, then build with
`--config .cargo/release-paths.toml` (git-ignored: it maps the cargo home, the checkout, the target dir and
`LK_CUSTOM_WEBRTC` to `/cargo`, `/tinhat`, `/target`, `/webrtc`). MSVC has no prefix map for C/C++, so on Windows also
build from a neutral folder with a neutral `CARGO_HOME` (e.g. `C:\tinhat\src`, `C:\cargo`). Don't use the `RUSTFLAGS`
variable for this: it replaces the repository's own rustflags.

**Windows release gate (every release):** the candidate must update itself: `windows\update-swap-gate.ps1` (a lab build
of the candidate → the same build as the next version, on a local feed signed by `scripts/update-swap-feed.sh`) must
print PASS, and `TINHAT_SAFE_UNZIP_BUNDLE=<the final win64 zip> cargo test --release -p tinhat-app
safe_unzip::tests::real_bundle -- --ignored` must pass (it fails once the bundle uses half of an unzip limit).

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
