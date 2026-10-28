# verify-artifact.ps1 - Windows RELEASE GATE: inspect the built artifact itself.
#
# This exists because two whole features/qualities have already shipped (or nearly
# shipped) broken while every build step reported success:
#   (1)    watch party was compiled OUT (build.rs probe missed MpvQt) yet the bundle
#          still shipped MpvQt.dll + libmpv-2.dll, so it LOOKED complete.
#   (2)    every non-ASCII UI character was mojibake because MSVC transcoded
#          qmlcachegen's narrow UTF-8 literals from the system ANSI codepage.
# Neither is visible in build output. Both are visible in the artifact. So check the
# artifact, not the log.
#
# ASCII-ONLY by rule: PowerShell 5.1 reads a BOM-less UTF-8 script as ANSI, so any
# non-ASCII literal here would itself be mojibake. Non-ASCII is expressed as bytes.
#
#   powershell -ExecutionPolicy Bypass -File windows\verify-artifact.ps1 -Exe <path>
# Exit code 0 = artifact is publishable, non-zero = do NOT publish.
param(
    [Parameter(Mandatory=$true)][string]$Exe
)
$ErrorActionPreference = "Continue"
if (-not (Test-Path $Exe)) { "VERIFY FAIL: no artifact at $Exe"; exit 2 }

$bytes = [IO.File]::ReadAllBytes($Exe)
# Latin1 maps bytes 1:1 to chars, so IndexOf over this is a fast raw byte search.
$enc = [Text.Encoding]::GetEncoding(28591)
$blob = $enc.GetString($bytes)
$fail = 0

function Seq([byte[]]$b) { return $enc.GetString($b) }

# ---- CHECK 1: UTF-8 decoded as CP1252 ----------------------------------------
# UTF-8 lead byte E2 (the whole U+2000 punctuation block: en/em dash, quotes,
# arrows, bullet) decoded as CP1252 becomes U+00E2 followed by a char that CP1252
# maps into U+2000-U+20AC. In UTF-16LE that is "E2 00" then "?? 20".
# Likewise lead byte C2 (U+00A0-U+00BF: middot, nbsp, degree) becomes U+00C2
# followed by U+00A0-U+00BF -> "C2 00" then "?? 00" with ?? in A0..BF.
$moji = @()
# A raw byte-pattern match alone is not evidence: 110 MB of compiled code contains
# these two bytes by coincidence constantly. Require the hit to sit inside a run of
# real TEXT - at least half of the surrounding 40 UTF-16 units printable ASCII -
# so only strings a user could actually read are reported.
function LooksLikeText([int]$at) {
    $lo = [Math]::Max(0, $at - 40 * 2)
    $hi = [Math]::Min($bytes.Length - 2, $at + 40 * 2)
    $printable = 0; $total = 0
    for ($k = $lo; $k -lt $hi; $k += 2) {
        $total++
        if ($bytes[$k+1] -eq 0x00 -and $bytes[$k] -ge 0x20 -and $bytes[$k] -le 0x7E) { $printable++ }
    }
    return ($total -gt 0 -and ($printable / $total) -ge 0.5)
}
for ($i = 0; $i -lt $bytes.Length - 3; $i++) {
    if ($bytes[$i+1] -ne 0x00) { continue }
    $cand = $false
    if ($bytes[$i] -eq 0xE2 -and $bytes[$i+3] -eq 0x20) { $cand = $true }
    elseif ($bytes[$i] -eq 0xC2 -and $bytes[$i+3] -eq 0x00 -and $bytes[$i+2] -ge 0xA0 -and $bytes[$i+2] -le 0xBF) { $cand = $true }
    if ($cand -and (LooksLikeText $i)) { $moji += $i; if ($moji.Count -ge 5) { break } }
}
if ($moji.Count -gt 0) {
    "VERIFY FAIL: mojibake in artifact - UTF-8 text was compiled as CP1252."
    "  This means the /utf-8 flag did not reach the compiler. Check crates/app/build.rs"
    "  (cc_builder -> cc.flag('/utf-8')) and that build.rs actually re-ran."
    foreach ($o in $moji) {
        $end = [Math]::Min($o + 60, $bytes.Length - 1)
        $txt = [Text.Encoding]::Unicode.GetString($bytes[$o..$end])
        "  at offset ${o}: $txt"
    }
    $fail = 1
} else {
    "verify ok: no CP1252-decoded UTF-8 found (mojibake check clean)"
}

# ---- CHECK 2: optional features are actually LINKED, not just bundled ----------
$dumpbin = (Get-ChildItem "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC" `
    -Recurse -Filter dumpbin.exe -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -like "*Hostx64\x64*" } | Select-Object -First 1).FullName
if (-not $dumpbin) {
    "verify WARN: dumpbin not found - cannot prove feature linkage"
} else {
    $imports = & $dumpbin /imports $Exe 2>$null
    # The exe links MpvQt.dll, not libmpv directly - MpvQt wraps libmpv, so MpvQt.dll
    # in the import table is what actually proves watch party was compiled in.
    if ($imports | Select-String -Quiet -Pattern "MpvQt\.dll") {
        "verify ok: watch party is LINKED (MpvQt.dll import present)"
    } else {
        "VERIFY FAIL: watch party is NOT linked (no MpvQt.dll import) - build.rs probe"
        "  missed MpvQt. A file on disk does not make cargo re-run build.rs: set"
        "  TINHAT_MPVQT_PREFIX and 'cargo clean -p tinhat-app' to force the probe."
        $fail = 1
    }
    # Linkage is necessary but not sufficient: MpvQt.dll loads libmpv at runtime and
    # the ytdl_hook shells out to yt-dlp, so a bundle can link fine and still fail on a
    # user's machine if those are absent next to the exe.
    $bundle = Split-Path $Exe -Parent
    foreach ($need in @("MpvQt.dll", "libmpv-2.dll", "yt-dlp.exe")) {
        if (Test-Path (Join-Path $bundle $need)) {
            "verify ok: bundle ships $need"
        } else {
            "VERIFY FAIL: bundle is missing $need - watch party will break at runtime"
            $fail = 1
        }
    }
    if ($imports | Select-String -Quiet -Pattern "gstreamer|gstapp") {
        "verify ok: GStreamer is LINKED"
    } else {
        "verify WARN: no GStreamer import seen - video path may be stubbed"
    }
}

# The NVENC AV1 chimera lesson: the exe must carry the C++ patch markers, or the build
# silently shipped stale webrtc-sys (new Rust + old C++ passed every other gate
# and got published for eleven minutes). ASCII scan; each marker is a distinct
# compiled-in feature. NOTE for future checks: never use cmd findstr with
# %ERRORLEVEL% on one line - it expands at parse time and always reads 0.
# Convert-Path: .NET resolves relative paths against the PROCESS cwd, not the
# PowerShell location — ReadAllText($Exe) returned null and the marker loop
# crashed per-iteration WITHOUT failing the gate (it said PASS around the
# wreckage). Fail CLOSED: if the bytes cannot be read, the artifact cannot be
# verified, and unverifiable means unpublishable.
$exeText = $null
try {
    $exeText = [System.IO.File]::ReadAllText((Convert-Path $Exe), [System.Text.Encoding]::ASCII)
} catch {
    "verify FAIL: cannot read artifact for marker scan: $_"
    $fail++
}
if (-not $exeText) { $exeText = "" }
foreach ($m in @(
    @{ tag = "[bwe-seed]";               why = "BWE estimator seed (6 Mbps crawl fix)" },
    @{ tag = "SetRates -> reconfigured"; why = "NVENC live rate reconfigure (10 Mbps pin fix)" },
    @{ tag = "nvenc-av1";                why = "NVENC AV1 encoder" },
    @{ tag = "but NVENC is unavailable"; why = "NVENC stack compiled in (CUDA_PATH was set)" },
    @{ tag = "their start is in effect";  why = "SAS start-race accept (verify stall fix)" },
    @{ tag = "secrets landed";            why = "post-SAS verified re-check (stale banner fix)" },
    @{ tag = "browser viewer in the call"; why = "H.264 share fallback when a browser is watching" },
    @{ tag = "exclude-process-tree";      why = "audio share excludes our own call playback" },
    @{ tag = "dev.tinhat.guests.waiting"; why = "the guest room: guests wait at the door until someone lets them in (docs/GUESTS.md)" },
    @{ tag = "record-optout";             why = "never-record-me consent pref (0.8.7)" },
    @{ tag = "dev.tinhat.record";         why = "Memories recording marker + room toggle (0.8.7)" },
    @{ tag = "dev.tinhat.clip";           why = "DVR clip bookmarks (0.8.8)" },
    @{ tag = "/vods/sessions";            why = "Memories list via the server's vods API (0.8.8)" }
)) {
    if ($exeText.Contains($m.tag)) {
        "verify ok: marker '$($m.tag)' present ($($m.why))"
    } else {
        "verify FAIL: marker '$($m.tag)' MISSING - $($m.why) not compiled in"
        $fail++
    }
}

# The signed updater (12.0.3, H3): a published exe trusts exactly Tin Hat's release key. A LAB build
# (--features lab-update-key) carries the TINHAT-LAB-BUILD marker and a development key and must never ship
# (gate 9, 2026-10-26: this script used to call such an exe publishable). Same rule as
# the release tooling's sign-desktop-feeds: no marker, the release key present, and no other embedded
# minisign public key (any 56-character "RW..." that decodes to 42 bytes starting with "Ed").
$releaseKey = "RWQUG5ce+eVTK+HZgYoCViu7WY1nwnbLR8s54/zRQR0DLiACjVuh5Mkv"   # key id 2B53E5F91E971B14
if ($exeText.Contains("TINHAT-LAB-BUILD")) {
    "verify FAIL: TINHAT-LAB-BUILD marker present - a LAB build that trusts a development update key"
    $fail++
}
# Calls round M1 (2026-10-27): a LAB build (--features lab-call-host) answers the "call hosted elsewhere" sheet from
# the TINHAT_TEST_CALL_HOST environment variable - the user's consent bypassed. A release build never reads it, so its
# name is never in a publishable exe (same refusal as sign-desktop-feeds).
if ($exeText.Contains("TINHAT_TEST_CALL_HOST")) {
    "verify FAIL: TINHAT_TEST_CALL_HOST present - a LAB build (--features lab-call-host) that answers the call-host sheet by itself"
    $fail++
} else {
    "verify ok: no lab call-host answer (TINHAT_TEST_CALL_HOST absent)"
}
# Every start position (a lookahead: overlapping candidates too, so one "RW..." can't hide the next), and an ordinal,
# case-sensitive set: base64 is case-sensitive, PowerShell's @{} and -ne are not (Astra, 12.0.3 train).
$keys = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::Ordinal)
foreach ($k in [regex]::Matches($exeText, "(?=(RW[A-Za-z0-9+/]{54}))")) {
    $v = $k.Groups[1].Value
    try { $b = [Convert]::FromBase64String($v) } catch { continue }
    if ($b.Length -eq 42 -and $b[0] -eq 0x45 -and $b[1] -eq 0x64) { [void]$keys.Add($v) }
}
if ($keys.Contains($releaseKey)) {
    "verify ok: the release update key is compiled in"
} else {
    "verify FAIL: the release update key is MISSING - built before the signed updater, or not ours"
    $fail++
}
$others = @($keys | Where-Object { $_ -cne $releaseKey })
if ($others.Count -gt 0) {
    "verify FAIL: another minisign public key is compiled in besides the release key: $($others -join ', ')"
    $fail++
}

if ($fail -ne 0) { "VERIFY: ARTIFACT IS NOT PUBLISHABLE"; exit 1 }
"VERIFY PASS: artifact is publishable"
exit 0
