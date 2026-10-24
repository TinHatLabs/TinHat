# dev-env.ps1 — Tin Hat Windows build environment (BUILD_WINDOWS.md §10)
# Dot-source from any PowerShell:  . .\windows\dev-env.ps1
# Pins: Qt 6.8.3 msvc2022_64 (aqt, C:\Qt) · GStreamer 1.28.4 MSVC (Program Files) · MSVC 14.44

# Qt — drives BOTH cxx-qt-build (qt-build-utils finds Qt ONLY via QMAKE/PATH, never
# CMAKE_PREFIX_PATH — BUILD_WINDOWS.md §4.2) AND crates/app/build.rs's qmake -query.
$env:QMAKE = "C:\Qt\6.8.3\msvc2022_64\bin\qmake.exe"
$env:Path = "C:\Qt\6.8.3\msvc2022_64\bin;" + $env:Path

# GStreamer 1.28.4 (unified installer set GSTREAMER_1_0_ROOT_MSVC_X86_64 machine-wide;
# resolve it here so the script works in a fresh shell too).
$gst = [Environment]::GetEnvironmentVariable('GSTREAMER_1_0_ROOT_MSVC_X86_64','Machine')
if (-not $gst) { $gst = "C:\Program Files\gstreamer\1.0\msvc_x86_64" }
$env:GSTREAMER_1_0_ROOT_MSVC_X86_64 = $gst
$env:Path = "$gst\bin;" + $env:Path
$env:PKG_CONFIG_PATH = "$gst\lib\pkgconfig"
# No standalone pkgconf on winget — GStreamer's devel install ships its own pkg-config.exe;
# the pkg-config crate honors PKG_CONFIG. (CHANGELOG deviation #3, resolved.)
$env:PKG_CONFIG = "$gst\bin\pkg-config.exe"

# MSVC (cl/link/SDK) — equivalent of the "x64 Native Tools" prompt, importable here.
$vsBase = "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools"
if (Test-Path "$vsBase\VC\Auxiliary\Build\vcvars64.bat") {
    cmd /c "`"$vsBase\VC\Auxiliary\Build\vcvars64.bat`" >nul 2>&1 && set" | ForEach-Object {
        if ($_ -match '^([^=]+)=(.*)$') { Set-Item -Path "Env:$($Matches[1])" -Value $Matches[2] }
    }
}

# Kirigami v6.24.0 (source-built to C:\kde\install — CHANGELOG deviation #2): the app
# imports org.kde.kirigami at RUNTIME, so the engine needs the module on the QML import
# path and its DLLs (Kirigami*.dll) loadable. Without this the production (qrc) load
# fails SILENTLY — no window, no error on stderr; only a file:// load via
# TINHAT_QML_OVERRIDE prints the real "module org.kde.kirigami is not installed".
$env:QML_IMPORT_PATH = "C:\kde\install\lib\qml"
$env:Path = "C:\kde\install\bin;" + $env:Path

# libmpv (watch-party): MpvQt.dll lives in C:\kde\install\bin (already on PATH
# above); the libmpv-2.dll it links lives with the shinchiro dev package.
$env:Path = "C:\kde\dist\mpv-dev;" + $env:Path
# CUDA (webrtc-sys NVENC windows arm reads CUDA_PATH; installer sets it machine-wide —
# resolve here so a fresh shell works even before reboot/re-login).
if (-not $env:CUDA_PATH) {
    $cudaRoot = "C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA"
    if (Test-Path $cudaRoot) {
        $v = Get-ChildItem $cudaRoot -Directory | Sort-Object Name -Descending | Select-Object -First 1
        if ($v) { $env:CUDA_PATH = $v.FullName }
    }
}

# Rust (winget rustup puts cargo on the user PATH; re-add for odd shells).
$env:Path = "$env:USERPROFILE\.cargo\bin;" + $env:Path

Write-Host "Tin Hat dev env ready: Qt $((& $env:QMAKE -query QT_VERSION)), GStreamer at $gst"
