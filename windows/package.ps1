# package.ps1 — assemble the portable Windows bundle, its zip and the installer
# (PACKAGING.md §7, adapted to the build machine's real prefixes). It never uploads anything.
#
#   .\windows\package.ps1                      build + windeployqt + bundle + zip + installer
#   .\windows\package.ps1 -SkipBuild           reuse target\release\tinhat.exe as-is
#   .\windows\package.ps1 -VersionOverride X   stamp VERSION/zip name as X (local updater tests)
#   .\windows\package.ps1 -UrlBase URL         the folder target\latest-windows.json points into
#                                              (default: this version's GitHub release,
#                                              https://github.com/TinHatLabs/TinHat/releases/download/v<ver>/;
#                                              a local test feed passes its own, e.g.
#                                              http://127.0.0.1:8765/)
#   TINHAT_SIGN_CMD (env, optional)            code-signing command; the file path is
#                                              appended, e.g. 'signtool sign /fd sha256
#                                              /tr http://timestamp.digicert.com /td sha256
#                                              /a'. Applied to the exe (before zipping)
#                                              and to the installer. Unsigned = SmartScreen
#                                              once per fresh download.
#
# Publishing: Windows releases go to GitHub only, as assets of the TinHatLabs/TinHat release
# v<ver> next to the Linux ones: tinhat-<ver>-win64.zip (the in-app updater's payload),
# TinHat-Setup-<ver>.exe (plus a fixed-name TinHat-Setup.exe copy for the download page) and
# latest-windows.json, the manifest the app reads from
# https://github.com/TinHatLabs/TinHat/releases/latest/download/latest-windows.json. The
# maintainers' release tool uploads them with the other platforms' files; the manifest it writes
# has the shape of target\latest-windows.json below.
#
# Deviations from PACKAGING.md §7 (logged in the working-folder CHANGELOG):
#   * Kirigami comes from the local source build (C:\kde\install), not KDE Craft.
#   * Watch-party (bare-metal): MpvQt.dll rides the $KdeRoot\bin\*.dll copy; libmpv-2.dll
#     (shinchiro, C:\kde\dist\mpv-dev) + yt-dlp.exe are staged in step 3c. mpv finds
#     yt-dlp via PATH (platform.rs prepends the exe dir when yt-dlp.exe is bundled).
#   * QQC2 style is forced to Basic in-app; breeze icons ship as the two rcc bundles
#     (step 3b) — no qqc2-breeze-style.
#   * No gstqml6/gstopengl (QVideoSink display path — PACKAGING.md §3 "recommended").
#   * Zip contains the BUNDLE CONTENTS at top level (UPDATE_PATH.md §1 shape, which the
#     in-app updater unzips into staging\current\; it also tolerates a wrapper dir).
param(
    [switch]$SkipBuild,
    [string]$VersionOverride = "",
    [string]$UrlBase = "",
    [string]$Notes   = ""
)
$ErrorActionPreference = "Stop"

$Repo    = Split-Path $PSScriptRoot -Parent
# Build env (QMAKE, GStreamer, vcvars64 -> dumpbin on PATH) — needed even with
# -SkipBuild for the dumpbin transitive-DLL walk below.
. "$Repo\windows\dev-env.ps1" | Out-Null
$QtBin   = "C:\Qt\6.8.3\msvc2022_64\bin"
$GstRoot = [Environment]::GetEnvironmentVariable('GSTREAMER_1_0_ROOT_MSVC_X86_64','Machine')
if (-not $GstRoot) { $GstRoot = "C:\Program Files\gstreamer\1.0\msvc_x86_64" }
$KdeRoot = "C:\kde\install"

# Version: crates/app/Cargo.toml is the source of truth (UPDATE_PATH.md §7: bump it in
# the same commit you publish); -VersionOverride only for local updater-cycle testing.
$Ver = $VersionOverride
if (-not $Ver) {
    $line = Select-String -Path "$Repo\crates\app\Cargo.toml" -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
    $Ver = $line.Matches[0].Groups[1].Value
}
$Stage = "$Repo\target\win-bundle\current"
$Zip   = "$Repo\target\tinhat-$Ver-win64.zip"

# ---- 1. build ----
if (-not $SkipBuild) {
    Push-Location $Repo
    . "$Repo\windows\dev-env.ps1"
    cargo build --release -p tinhat-app
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
    Pop-Location
}

# ---- 2. stage + windeployqt ----
Remove-Item -Recurse -Force "$Repo\target\win-bundle" -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $Stage | Out-Null
# The in-app updater only accepts a zip with tinhat.exe at its top level (platform.rs
# install_update_zip); the installer's shortcuts open <root>\current\tinhat.exe.
Copy-Item "$Repo\target\release\tinhat.exe" "$Stage\tinhat.exe"
# PS 5.1 wraps native stderr into ErrorRecords and $ErrorActionPreference=Stop turns a
# benign windeployqt WARNING into a terminating error — relax around the call; the exit
# code is the real verdict.
$eap = $ErrorActionPreference; $ErrorActionPreference = "Continue"
& "$QtBin\windeployqt.exe" --release --compiler-runtime --qmldir "$Repo\crates\app\qml" "$Stage\tinhat.exe" 2>&1 |
    ForEach-Object { "$_" } | Select-Object -Last 5
$ErrorActionPreference = $eap
if ($LASTEXITCODE -ne 0) { throw "windeployqt failed" }

# ---- 3. Kirigami (local source build): QML module + runtime DLLs ----
New-Item -ItemType Directory -Force "$Stage\qml\org\kde" | Out-Null
Copy-Item "$KdeRoot\lib\qml\org\kde\kirigami" "$Stage\qml\org\kde\kirigami" -Recurse -Force
Copy-Item "$KdeRoot\bin\*.dll" $Stage -Force    # Kirigami*.dll etc. (flat, exe-dir search)

# ---- 3b. Breeze icon theme (binary resources the C++ shim in video_bridge.cpp registers) ----
# Two rcc bundles (light = fallback/inheritance target, dark = the app's theme) built
# from frameworks/breeze-icons v6.24 (C:\kde\build\breeze-icons). Without these Windows
# has NO icon theme and every icon.name renders blank.
$iconsBuild = "C:\kde\build\breeze-icons\icons"
New-Item -ItemType Directory -Force "$Stage\icons" | Out-Null
foreach ($rcc in @("breeze-icons.rcc", "breeze-icons-dark.rcc")) {
    if (Test-Path "$iconsBuild\$rcc") { Copy-Item "$iconsBuild\$rcc" "$Stage\icons\" -Force }
    else { Write-Warning "missing icon bundle: $iconsBuild\$rcc" }
}
# 0.8.26: the Tin Hat icon pack (static theme + animated webp) — iconpack.rs looks for
# <exe dir>\iconpack in a bundle. Absent pack = Breeze only, exactly as before.
$iconPack = "$Repo\crates\app\assets\iconpack"
if (Test-Path $iconPack) { Copy-Item $iconPack "$Stage\iconpack" -Recurse -Force }

# ---- 3c. Watch-party (libmpv + yt-dlp) + exe icon file ----
# MpvQt.dll came with the $KdeRoot\bin copy above. libmpv-2.dll is the C-ABI mpv
# runtime MpvQt links; yt-dlp.exe is found via PATH by mpv's ytdl_hook (platform.rs
# prepends the exe dir). tinhat.ico feeds the AUMID IconUri (toast attribution)
# and the (future) installer shortcut.
foreach ($extra in @(
    @{ src = "C:\kde\dist\mpv-dev\libmpv-2.dll"; why = "watch-party (libmpv runtime)" },
    @{ src = "C:\kde\dist\yt-dlp.exe";           why = "watch-party (ytdl_hook)" },
    @{ src = "$Repo\crates\app\tinhat.ico"; why = "toast/shortcut icon" }
)) {
    if (Test-Path $extra.src) { Copy-Item $extra.src $Stage -Force }
    else { Write-Warning "missing $($extra.why): $($extra.src)" }
}

# ---- 4. GStreamer: core DLLs + plugins + scanner ----
$plugins = @(
  # call media (PACKAGING.md §2 list): wasapi2 mic/playback, mfvideosrc camera,
  # audiobuffersplit+audiorate (gstaudiobuffersplit/gstaudiorate live in coreelements/
  # audiorate DLLs), gstrsaudiofx = audiornnoise (RNNoise noise-cancel), jpeg = jpegdec
  # (MJPEG cams), videorate for the tile/camera chains.
  "gstcoreelements","gstapp","gstvideoconvertscale","gstvideorate","gstvideotestsrc",
  "gstaudiotestsrc","gstaudioconvert","gstaudioresample","gstaudiorate","gstaudiobuffersplit",
  "gstaudioparsers","gsttypefindfunctions","gstjpeg","gstrsaudiofx","gstvolume",
  "gstplayback","gstsoup","gstd3d11","gstmediafoundation","gstwasapi","gstwasapi2","gstnvcodec",
  # webrtcdsp = the Windows echo-canceller/AGC (webrtcdsp + webrtcechoprobe — call.rs win AEC);
  # d3d11 above also carries d3d11screencapturesrc/d3d11download for screen share.
  "gstwebrtcdsp",
  # AUDIO DECODE for soundboard/music/chimes (found live 2026-08-09: EVERY user's
  # soundboard was silent — the https fetch worked post-TLS-fix but uridecodebin had
  # NO mp3/aac decoder in the bundle; a developer's box silently fell back to the
  # system GStreamer install, masking it). gstlibav = ffmpeg decoders (mp3float/aac/
  # everything); mpg123 as a lean mp3 fallback; demuxers/parsers for m4a/ogg/flac/wav.
  "gstlibav","gstmpg123","gstisomp4","gstogg","gstvorbis","gstopus","gstflac","gstwavparse",
  # TAG DEMUXERS (found in a field log): typefind classifies any
  # mp3 with an ID3v2 header as application/x-id3 — which is EVERY myinstants clip —
  # and without id3demux uridecodebin dies with "Missing decoder: ID3 tag" the moment
  # a soundboard clip is published into a call (the local QtMultimedia preview plays,
  # so the sender never knows peers heard nothing). apetag for APE-tagged files;
  # matroska for webm/ogg-opus web sounds.
  "gstid3demux","gstapetag","gstmatroska"
)
New-Item -ItemType Directory -Force "$Stage\lib\gstreamer-1.0" | Out-Null
foreach ($p in $plugins) {
    $dll = "$GstRoot\lib\gstreamer-1.0\$p.dll"
    if (Test-Path $dll) { Copy-Item $dll "$Stage\lib\gstreamer-1.0\" }
    else { Write-Warning "missing gst plugin: $p" }
}
# gio TLS/proxy modules (glib-networking): souphttpsrc needs these for ANY https
# fetch (soundboard/music/notification sounds). They are RUNTIME-loaded via gio,
# invisible to the dumpbin import walk — stage explicitly, then seed the walk so
# their own deps (libssl/libcrypto/proxy) get pulled too. glib resolves
# <prefix>\lib\gio\modules relative to the glib DLL; platform.rs also pins
# GIO_MODULE_DIR explicitly. (Found live: soundboard error -5 on every https
# sound — 2026-08-08, first bare-metal session.)
$gioSrc = "$GstRoot\lib\gio\modules"
if (Test-Path $gioSrc) {
    New-Item -ItemType Directory -Force "$Stage\lib\gio\modules" | Out-Null
    # giolibproxy is EXCLUDED: its libproxy dependency is not shipped, so gio logs a
    # "Failed to load module" on the first https fetch (every session, 2026-08-19 log);
    # nothing here uses OS proxy autodetection. Ship libproxy+deps instead if that ever
    # changes.
    Copy-Item "$gioSrc\*.dll" "$Stage\lib\gio\modules\" -Force -Exclude "giolibproxy.dll"
} else { Write-Warning "gio modules dir missing: $gioSrc" }

$scanner = "$GstRoot\libexec\gstreamer-1.0\gst-plugin-scanner.exe"
if (Test-Path $scanner) {
    New-Item -ItemType Directory -Force "$Stage\libexec\gstreamer-1.0" | Out-Null
    Copy-Item $scanner "$Stage\libexec\gstreamer-1.0\"
} else { Write-Warning "gst-plugin-scanner.exe not found at $scanner" }
# Transitive core DLLs: walk the bundled plugins' imports against the GStreamer bin dir
# (PACKAGING.md §2 "walk each plugin's imports rather than guessing"). Iterate until
# closed (DLLs pull DLLs).
$gstBin = "$GstRoot\bin"
$have   = @{}
Get-ChildItem "$Stage\*.dll" | ForEach-Object { $have[$_.Name.ToLower()] = $true }
$queue  = @(Get-ChildItem "$Stage\lib\gstreamer-1.0\*.dll") + @(Get-ChildItem "$Stage\libexec\gstreamer-1.0\*.exe") + @(Get-ChildItem "$Stage\lib\gio\modules\*.dll" -ErrorAction SilentlyContinue)
$seen   = @{}
while ($queue.Count -gt 0) {
    $item = $queue[0]; $queue = $queue[1..$queue.Count]
    if ($seen[$item.FullName]) { continue }; $seen[$item.FullName] = $true
    $deps = & dumpbin /dependents $item.FullName 2>$null | Select-String '^\s+\S+\.dll' | ForEach-Object { $_.Line.Trim().ToLower() }
    foreach ($d in $deps) {
        if ($have[$d]) { continue }
        $src = Join-Path $gstBin $d
        if (Test-Path $src) {
            Copy-Item $src $Stage
            $have[$d] = $true
            $queue += Get-Item (Join-Path $Stage $d)
        }
    }
}

# ---- 5. VERSION (updater bootstrap sanity — UPDATE_PATH.md §3.2) ----
Set-Content -Path "$Stage\VERSION" -Value $Ver -Encoding ascii -NoNewline

# ---- 5b. code signing (optional; see header) ----
function Sign-IfConfigured([string]$path) {
    if (-not $env:TINHAT_SIGN_CMD) { return }
    Write-Host "signing: $path"
    Invoke-Expression "$($env:TINHAT_SIGN_CMD) `"$path`""
    if ($LASTEXITCODE -ne 0) { throw "code signing failed: $path" }
}
Sign-IfConfigured "$Stage\tinhat.exe"

# ---- 6. zip (bundle CONTENTS at top level) ----
Remove-Item $Zip -ErrorAction SilentlyContinue
Compress-Archive -Path "$Stage\*" -DestinationPath $Zip
$Sha = (Get-FileHash $Zip -Algorithm SHA256).Hash.ToLower()

# ---- 6b. installer (Inno Setup 6, per-user, no admin — windows/installer.iss) ----
# The friendly first-run vehicle: one file, Welcome → Install → Finish, shortcuts, and
# the exact self-updater layout so the in-app updater owns everything afterwards.
$Setup = "$Repo\target\TinHat-Setup-$Ver.exe"
$SetupSha = ""
# The installer's Repair/Uninstall only take files some release shipped out of folders that
# also hold other things (windows/INSTALLER.md): pack the committed list plus this bundle's
# files. Written under target\ - a changed tracked file would block the VM's next git pull.
$ListIn  = "$Repo\windows\tinhat-bundle-files.txt"
$ListOut = "$Repo\target\tinhat-bundle-files.txt"
$known = [ordered]@{}
$header = @(Get-Content $ListIn | Where-Object { $_.StartsWith('#') })
Get-Content $ListIn | Where-Object { $_ -and -not $_.StartsWith('#') } | ForEach-Object { $known[$_.ToLower()] = $_ }
Get-ChildItem -Recurse -File $Stage | ForEach-Object {
    $rel = $_.FullName.Substring($Stage.Length + 1)
    if (-not $known.Contains($rel.ToLower())) { $known[$rel.ToLower()] = $rel }
}
[System.IO.File]::WriteAllLines($ListOut, [string[]]($header + @($known.Values | Sort-Object)), (New-Object System.Text.UTF8Encoding($false)))
$Iscc = @("$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
          "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
          "$env:ProgramFiles\Inno Setup 6\ISCC.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1
if ($Iscc) {
    Remove-Item $Setup -ErrorAction SilentlyContinue
    $eap = $ErrorActionPreference; $ErrorActionPreference = "Continue"
    & $Iscc /Qp "/DAppVer=$Ver" "/DBundleList=$ListOut" "$Repo\windows\installer.iss" 2>&1 | ForEach-Object { "$_" } | Select-Object -Last 3
    $ErrorActionPreference = $eap
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path $Setup)) { throw "ISCC failed - installer not built" }
    Sign-IfConfigured $Setup
    $SetupSha = (Get-FileHash $Setup -Algorithm SHA256).Hash.ToLower()
} else {
    Write-Warning "Inno Setup (ISCC.exe) not found - no installer built this run (zip only). winget install JRSoftware.InnoSetup"
}

# ---- 7. manifest (target\latest-windows.json, the shape the GitHub release carries) ----
# `url`/`sha256` stay the ZIP (the self-updater's payload — never point them at the
# installer). `installer`/`sha256_installer` are additive: the download page reads them,
# the app ignores them. The URLs point into -UrlBase, by default this version's GitHub
# release; a local updater test serves target\ itself and passes its own -UrlBase.
if (-not $UrlBase) { $UrlBase = "https://github.com/TinHatLabs/TinHat/releases/download/v$Ver/" }
if (-not $UrlBase.EndsWith('/')) { $UrlBase += '/' }
$installerFields = ""
if ($SetupSha) {
    $installerFields = @"
  "installer": "$UrlBase$(Split-Path $Setup -Leaf)",
  "sha256_installer": "$SetupSha",

"@
}
$manifest = @"
{
  "version": "$Ver",
  "url": "$UrlBase$(Split-Path $Zip -Leaf)",
  "sha256": "$Sha",
$installerFields  "notes": "$Notes"
}
"@
# BOM-less UTF-8 (PS 5.1 `-Encoding utf8` writes a BOM, which serde_json rejects —
# caught live by the TINHAT_UPDATE_TEST cycle).
[System.IO.File]::WriteAllText("$Repo\target\latest-windows.json", $manifest, (New-Object System.Text.UTF8Encoding($false)))
Write-Host "bundle : $Stage"
Write-Host "zip    : $Zip"
Write-Host "sha256 : $Sha"
if ($SetupSha) { Write-Host "setup  : $Setup"; Write-Host "sha256 : $SetupSha" }
Write-Host "manifest: $Repo\target\latest-windows.json (urls under $UrlBase)"
Write-Host "Nothing was uploaded: Windows releases are published as GitHub release assets (see the header)."
