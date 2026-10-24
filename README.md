# Tin Hat

Use Tin Hat for anything, including streaming, communities and businesses. If you run a modified version for other people, share your changes (AGPL-3.0-or-later).

Tin Hat is a native desktop client for [Matrix](https://matrix.org) built for game nights: rooms
and spaces, end-to-end encrypted chat, and calls with **hardware-encoded, end-to-end encrypted
screen sharing** (NVENC, VA-API) at up to 4K60. Calls speak Element Call's protocol over LiveKit,
so phones on Element X join the same rooms.

Rust + CXX-Qt + QML (Kirigami). Linux ships as a Flatpak, Windows as an installer. The server side
is Tin Hat Server (`TinHatLabs/TinHatServer`): Synapse, LiveKit and a small API, on your own box.

- Licence: AGPL-3.0-or-later (`LICENSE`). Third-party components: `THIRD-PARTY-NOTICES.md`.
  The name and the mark: `TRADEMARK.md`.
- Building: `BUILDING.md` (Linux, Windows, Flatpak).
- Versions 0.1 – 9.4 shipped under an earlier name; the app became Tin Hat in 10.0.
