# Third-party notices

Tin Hat is AGPL-3.0-or-later (see LICENSE). It builds on the following components, which keep their
own licences. Nothing here is copyleft-incompatible; no GPL-only runtime library ships in the
bundle (libmpv and yt-dlp were dropped in 10.0).

## Linked into the binary (Rust crates, C++)

| Component | Licence |
|---|---|
| matrix-rust-sdk (Matrix client, E2EE) | Apache-2.0 |
| LiveKit client SDK, webrtc-sys, libwebrtc | Apache-2.0 / BSD-3-Clause (libwebrtc), with our patches under AGPL |
| CXX-Qt, cxx | MIT OR Apache-2.0 |
| tokio, serde, reqwest, rustls, ring/aws-lc-rs, zbus, ashpd and ~560 further crates | MIT, Apache-2.0, BSD, ISC, Zlib, Unicode-3.0, MPL-2.0 (as_variant, eyeball, readlock, imbl, bitmaps), CC0/MIT-0 (blake3, constant_time_eq, dunce), Unlicense — the full per-crate list: `cargo metadata` / `cargo license` in the source tree |
| NVIDIA Video Codec SDK headers (NvEncoder/NvDecoder samples) | MIT (samples), NVIDIA Software Licence (headers) |
| Boost (via libwebrtc build) | BSL-1.0 |

## Dynamically linked at runtime (system or Flatpak runtime)

| Component | Licence |
|---|---|
| Qt 6 (Core, Gui, Qml, Quick, Quick Shapes, Multimedia, WebEngine, Svg, 5Compat) | LGPL-3.0 (WebEngine also BSD-3-Clause, Chromium) |
| KDE Frameworks: Kirigami, KConfig, KColorScheme, QQC2 Desktop Style, Breeze icons, Layer Shell Qt | LGPL-2.1-or-later (icons: LGPL-3.0) |
| GStreamer + plugins (base, good, bad, libav, pipewire, qml6, rs/audiofx) | LGPL-2.1-or-later; libav plugin GPL-2.0-or-later when built with GPL FFmpeg — the Flatpak uses the runtime's LGPL FFmpeg |
| FFmpeg (through Qt Multimedia and gst-libav) | LGPL-2.1-or-later |
| PipeWire, libpulse | MIT / LGPL-2.1 |
| RNNoise (gst audiornnoise) | BSD-3-Clause |
| libva / Mesa VA-API drivers | MIT |
| glibc | LGPL-2.1 |

## Flatpak bundle: NVIDIA NVRTC

The Linux Flatpak carries NVIDIA's CUDA Runtime Compilation library, `libnvrtc.so.12` + `libnvrtc-builtins.so.12.9`
(cuda_nvrtc 12.9.86, unmodified, from https://developer.download.nvidia.com/compute/cuda/redist/), so GStreamer's
CUDA filters can scale screen shares on NVIDIA GPUs. It is proprietary NVIDIA software, redistributed under the CUDA
Toolkit End User License Agreement (Attachment A lists it as distributable); the full agreement ships at
`/app/share/licenses/nvrtc/LICENSE`. Tin Hat does not link it: GStreamer's nvcodec plugin loads it at runtime, and
nvcodec only uses it on machines with an NVIDIA driver (elsewhere it registers no CUDA elements). Not part of the Windows or pacman builds.

## Windows bundle (windeployqt)

The Windows zip and installer carry the Qt DLLs above (LGPL-3.0) plus the MSVC runtime
(Microsoft redistributable licence), and the KDE Breeze icon theme compiled into
`breeze-icons.rcc` / `breeze-icons-dark.rcc` (breeze-icons v6.24,
https://invent.kde.org/frameworks/breeze-icons, unmodified; LGPL-3.0-or-later, © 2014 Uri Herrera and
others — Linux uses the system's or the Flatpak runtime's Breeze instead). Qt's LGPL terms are met by dynamic linking; replace any Qt
DLL with your own build if you wish. Copies of the LGPL-3.0 and LGPL-2.1 texts are at
https://www.gnu.org/licenses/.

## Assets

- Tin Hat mark, loader and app icons: original vector artwork, © 2026 the Tin Hat authors — see TRADEMARK.md.
- OLED sound set (join, leave, message, notification, ringtone): royalty-free sound effects licensed for redistribution in this software, added 2026-09-16.
- OLED "muted" cue (oled/muted.mp3): generated (a two-tone sine down-step), no third-party material.
- Goose sound set (join, leave): short sound effects bundled with the Goose theme.
- Icon pack (crates/app/assets/iconpack): original artwork drawn for this project (not derived from Breeze; its theme
  only names Breeze as the fallback for icons it doesn't draw), © 2026 the Tin Hat authors.
- Emoji rendering uses the host's colour emoji font (Noto Color Emoji, OFL-1.1, where installed).
