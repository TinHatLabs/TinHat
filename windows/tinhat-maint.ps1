# tinhat-maint.ps1 - the installer's clean-up helper for Repair and Uninstall (2026-09-26).
#
# Tin Hat reached Windows PCs in several shapes over its history: zips extracted into
# Downloads and run from there (they never updated, and a copy left running held the
# single-instance lock so the installed app would not open), copies unzipped by hand into
# its data folder, shortcuts and taskbar pins pointing at those copies, a second
# Apps & features entry from the in-app "Install to your apps", Windows Firewall rules for
# each copy, crash reports. installer.iss removes the app's own fixed footprint itself;
# this helper FINDS the rest and removes or fixes what the user leaves ticked.
#
#   -Action close     close every Tin Hat of this user's session that really is Tin Hat:
#                     ask (WM_CLOSE), wait up to 12 s, then end it. Exit 128 = none was
#                     running, 0 = closed, 1 = had to be ended, 2 = still running. No WMI
#                     (taskkill needs it and fails where WMI is broken or the logon is not
#                     interactive).
#   -Action scan      write the findings to -Out, one per line:
#                        kind <TAB> ticked(0|1) <TAB> path <TAB> label <TAB> auto(0|1) <TAB> seen
#                     "seen" = what the item was at scan time (a copy's file count and bytes,
#                     a shortcut's target): apply acts only if it is still exactly that.
#                     "auto" = safe to act on when nobody reviews the list (the Apps &
#                     features uninstaller, silent runs): only the usual places a zip copy
#                     lives (Downloads, Desktop, Temp) and the app's own spots. A copy found
#                     anywhere else is removed only when a person saw it ticked on the page.
#   -Action apply     act on the findings listed in -In (same format, only the ticked ones);
#                     logs "done: <kind> <path>" or "FAILED: <kind> <path>" per item
#   -Action fwcheck   exit code = how many Windows Firewall rules still exist for the app
#                     paths listed in -In (the removal itself runs elevated, from code on
#                     the elevated command line - never from a file the user can change)
#   -Action signout   sign this PC's Tin Hat sessions out on the server (reads session.json;
#                     the token never appears on a command line or in the log)
#   -Action traces    forget what Windows recorded about Tin Hat (uninstall only)
#   -Action checkmeta exit 0 when Setup's own files in the data folder (unins*.dat/exe/msg,
#                     the lock, the helper files) are plain files - no link, no second hard
#                     link - that Inno and Setup may rewrite; 3 when one is not; 4 (or any
#                     other code) when that could not be verified.
#
# How it stays safe (the rules the gate checks, windows/INSTALLER.md):
#   * A folder is Tin Hat's as a whole only when EVERY file in it is a file some release
#     shipped (tinhat-bundle-files.txt) or one the app writes next to itself; otherwise only
#     those files leave it, one by one, and the folder stays with the user's own files.
#   * Nothing follows a junction or symbolic link: links are removed, never walked into.
#   * Copies and downloads in the user's folders go to the Recycle Bin. If Windows will not
#     recycle something it stays where it is - never a silent permanent delete.
#   * Known folders (Downloads, Desktop, the profile, drive roots ...) never go as a whole.
#
# Every step appends one plain line to -Log; the installer copies it into its Setup log.
# Keep this file ASCII: Windows PowerShell 5.1 reads a script without a BOM as ANSI.
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('close', 'scan', 'apply', 'fwcheck', 'signout', 'traces', 'checkmeta')]
    [string]$Action,
    [ValidateSet('install', 'repair', 'uninstall')]
    [string]$Mode = 'repair',
    [string]$Out = '',
    [string]$In = '',
    [string]$Log = '',
    # The installer that is running right now: never offered for deletion.
    [string]$Self = '',
    # Uninstall keeping the sign-in: sandboxed copies keep their data too.
    [switch]$KeepData
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2

$Utf8Bom = New-Object System.Text.UTF8Encoding($true)
$Utf8 = New-Object System.Text.UTF8Encoding($false)

# Tin Hat's data folder, also the self-updater's root (installer.iss RootDir, platform.rs
# WINDOWS_ROOT_NAME); current\ is the live bundle.
$Root = Join-Path $env:LOCALAPPDATA 'TinHatApp'
$Current = Join-Path $Root 'current'
$Installed = Join-Path $Current 'tinhat.exe'
$ExeNames = @('tinhat.exe')
# The toast identity (AppUserModelID) the installer and the app register.
$Aumid = 'dev.tinhat.TinHat'

# Every file a release bundle ever held, relative to the bundle folder.
$Manifest = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::Ordinal)
$ManifestFile = Join-Path $PSScriptRoot 'tinhat-bundle-files.txt'
$ManifestError = ''
try {
    if (Test-Path -LiteralPath $ManifestFile) {
        foreach ($l in [System.IO.File]::ReadAllLines($ManifestFile)) {
            $t = $l.Trim()
            if ($t -and -not $t.StartsWith('#')) { [void]$Manifest.Add($t) }
        }
    }
} catch { $Manifest.Clear(); $ManifestError = $_.Exception.Message }
# Generic names a user may well have downloaded on their own: never taken out of a folder
# that also holds other things (they only go with a whole Tin Hat folder).
$KeepInMixed = @('yt-dlp.exe', 'vc_redist.x64.exe', 'VERSION')
# Everything Tin Hat writes in its data folder (same list as installer.iss DeleteKnownRootData).
# Tin Hat owns each of these wholesale (the in-app updater already replaces current\ on every
# update); a file of the user's own NEXT to them stays.
$KnownDataNames = @('session.json', 'store', 'config', 'theme', 'themes', 'themesync', 'layoutsync', 'sameroom_pairs.json',
    'nav_mode', 'tour_seen', 'space_order', 'room_recency.json', 'calendar-seen.json', 'audio_prefs.json',
    'presence-pending-off.json', 'widgets-history.json', 'mentions.json', 'dice.json', 'clocks-tz.json', 'memories-thumbs',
    'assets', 'avatars', 'iconpack', 'logs', 'gstregistry.bin', 'install-prompt-seen', 'tinhat-maint.ps1',
    'tinhat-bundle-files.txt', 'current', 'backup', 'staging', 'run', 'setup-previous', 'setup-interrupted',
    'setup-interrupted-1', 'setup-interrupted-2', 'setup-interrupted-3')

function Write-Log([string]$Line) {
    if (-not $Log) { return }
    try { [System.IO.File]::AppendAllText($Log, "[maint] $Line`r`n", $Utf8) } catch { }
}

function Norm([string]$Path) {
    if (-not $Path) { return '' }
    try { $p = [System.IO.Path]::GetFullPath($Path) } catch { $p = $Path }
    if ($p.Length -gt 3) { $p = $p.TrimEnd('\') }
    return $p
}

function Same([string]$A, [string]$B) { return (Norm $A) -ieq (Norm $B) }

function Under([string]$Path, [string]$Dir) {
    $p = Norm $Path; $d = (Norm $Dir).TrimEnd('\')
    return $d -and ($p -ieq $d -or $p.StartsWith($d + '\', [System.StringComparison]::OrdinalIgnoreCase))
}

function Known-Folder([string]$Guid, [string]$Fallback) {
    try {
        $v = (Get-ItemProperty -LiteralPath 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\User Shell Folders' -Name $Guid -ErrorAction Stop).$Guid
        if ($v) { return [Environment]::ExpandEnvironmentVariables($v) }
    } catch { }
    return $Fallback
}

$Desktop = [Environment]::GetFolderPath('Desktop')
$Documents = [Environment]::GetFolderPath('MyDocuments')
$Programs = [Environment]::GetFolderPath('Programs')
$Startup = [Environment]::GetFolderPath('Startup')
$Downloads = Known-Folder '{374DE290-123F-4565-9164-39C4925E467B}' (Join-Path $env:USERPROFILE 'Downloads')
$QuickLaunch = Join-Path $env:APPDATA 'Microsoft\Internet Explorer\Quick Launch'
$TaskbarPins = Join-Path $QuickLaunch 'User Pinned\TaskBar'

# A readable place name for labels: "Downloads\tinhat-12.0.0-win64" rather than the full path.
function Friendly([string]$Path) {
    $p = Norm $Path
    foreach ($pair in @(@($Downloads, 'Downloads'), @($Desktop, 'Desktop'), @($Documents, 'Documents'),
            @($TaskbarPins, 'Taskbar'), @($Programs, 'Start menu'), @($env:TEMP, 'Temp'),
            @($env:LOCALAPPDATA, '%LOCALAPPDATA%'), @($env:APPDATA, '%APPDATA%'), @($env:USERPROFILE, '%USERPROFILE%'))) {
        $d = Norm $pair[0]
        if ($d -and (Under $p $d)) { return $pair[1] + $p.Substring($d.Length) }
    }
    return $p
}

function Clean-Label([string]$Text) { return ($Text -replace "[`t`r`n]", ' ') }

# ---------------------------------------------------------------------------------------
# Native bits: the Recycle Bin, and whether a reparse point is a link (junction/symlink)
# or just a cloud placeholder (OneDrive) - compiled once, on first use.
# ---------------------------------------------------------------------------------------

$NativeSource = @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class TinHatNative {
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode, Pack = 1)]
    struct Op32 { public IntPtr hwnd; public uint wFunc; public string pFrom; public string pTo; public ushort fFlags; public int fAnyOperationsAborted; public IntPtr hNameMappings; public string lpszProgressTitle; }
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct Op64 { public IntPtr hwnd; public uint wFunc; public string pFrom; public string pTo; public ushort fFlags; public int fAnyOperationsAborted; public IntPtr hNameMappings; public string lpszProgressTitle; }
    [DllImport("shell32.dll", CharSet = CharSet.Unicode, EntryPoint = "SHFileOperationW")] static extern int Sh32(ref Op32 op);
    [DllImport("shell32.dll", CharSet = CharSet.Unicode, EntryPoint = "SHFileOperationW")] static extern int Sh64(ref Op64 op);
    // FO_DELETE with ALLOWUNDO | NOCONFIRMATION | NOERRORUI | SILENT | WANTNUKEWARNING |
    // NO_CONNECTED_ELEMENTS: to the Recycle Bin without questions, never dragging along a
    // "connected" file (report.htm with report_files), and Windows asks the user before
    // anything would be deleted permanently. -2 = the user or Windows aborted part of it.
    public static int Recycle(string list) {
        const uint FO_DELETE = 3;
        const ushort Flags = 0x0040 | 0x0010 | 0x0400 | 0x0004 | 0x4000 | 0x2000;
        int rc; int aborted;
        if (IntPtr.Size == 4) { var a = new Op32(); a.wFunc = FO_DELETE; a.pFrom = list + "\0\0"; a.fFlags = Flags; rc = Sh32(ref a); aborted = a.fAnyOperationsAborted; }
        else { var b = new Op64(); b.wFunc = FO_DELETE; b.pFrom = list + "\0\0"; b.fFlags = Flags; rc = Sh64(ref b); aborted = b.fAnyOperationsAborted; }
        if (rc == 0 && aborted != 0) return -2;
        return rc;
    }
    // WIN32_FIND_DATAW exactly: FILETIMEs are pairs of DWORDs (4-byte aligned), so the
    // reparse tag (dwReserved0) sits at offset 36 and the struct is 592 bytes.
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct FindData { public uint Attributes; public uint C1, C2, A1, A2, W1, W2; public uint SizeHigh, SizeLow, Reserved0, Reserved1;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 260)] public string Name; [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 14)] public string ShortName; }
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true, EntryPoint = "FindFirstFileW")] static extern IntPtr FindFirst(string name, out FindData data);
    [DllImport("kernel32.dll")] static extern bool FindClose(IntPtr h);
    public static int FindDataSize() { return Marshal.SizeOf(typeof(FindData)); }
    public static int TagOffset() { return (int)Marshal.OffsetOf(typeof(FindData), "Reserved0"); }
    // True when the entry is a reparse point that names another place (junction, symbolic
    // link, mount point - IsReparseTagNameSurrogate). Unknown counts as a link.
    [StructLayout(LayoutKind.Sequential)]
    struct FileInfo { public uint Attributes; public uint C1, C2, A1, A2, W1, W2; public uint Serial, SizeHigh, SizeLow, Links, IndexHigh, IndexLow; }
    public static int FileInfoSize() { return Marshal.SizeOf(typeof(FileInfo)); }
    public static int LinksOffset() { return (int)Marshal.OffsetOf(typeof(FileInfo), "Links"); }
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool GetFileInformationByHandle(IntPtr h, out FileInfo info);
    // How many names (hard links) the file has; -1 if unknown.
    public static int LinkCount(string path) {
        IntPtr h = Open(path, 0, 7, IntPtr.Zero, 3, 0x02000000, IntPtr.Zero);
        if (h == new IntPtr(-1)) return -1;
        try { FileInfo fi; if (!GetFileInformationByHandle(h, out fi)) return -1; return (int)fi.Links; } finally { CloseHandle(h); }
    }
    public static bool IsLink(string path) {
        FindData d; IntPtr h = FindFirst(path, out d);
        if (h == new IntPtr(-1)) return true;
        FindClose(h);
        if ((d.Attributes & 0x400) == 0) return false;
        return (d.Reserved0 & 0x20000000) != 0;
    }
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true, EntryPoint = "CreateFileW")]
    static extern IntPtr Open(string name, uint access, uint share, IntPtr sec, uint disposition, uint flags, IntPtr template);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true, EntryPoint = "GetFinalPathNameByHandleW")]
    static extern uint FinalName(IntPtr h, StringBuilder buf, uint size, uint flags);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
    // Where a path really is: every junction, symbolic link and 8.3 short name on the way
    // resolved. null when it cannot be opened.
    public static string FinalPath(string path) {
        IntPtr h = Open(path, 0, 7, IntPtr.Zero, 3, 0x02000000, IntPtr.Zero);   // any access, share all, OPEN_EXISTING, BACKUP_SEMANTICS
        if (h == new IntPtr(-1)) return null;
        try {
            var sb = new StringBuilder(1024);
            uint n = FinalName(h, sb, (uint)sb.Capacity, 0);
            if (n == 0 || n >= sb.Capacity) return null;
            string s = sb.ToString();
            if (s.StartsWith(@"\\?\UNC\")) return @"\\" + s.Substring(8);
            if (s.StartsWith(@"\\?\")) return s.Substring(4);
            return s;
        } finally { CloseHandle(h); }
    }
}
'@
$NativeReady = $null
$RecycleTrouble = $false

function Use-Native {
    if ($null -eq $script:NativeReady) {
        try {
            Add-Type -TypeDefinition $NativeSource -ErrorAction Stop
            # A wrong struct layout would read the wrong field and call a junction a folder.
            if ([TinHatNative]::FindDataSize() -ne 592 -or [TinHatNative]::TagOffset() -ne 36) { throw 'WIN32_FIND_DATAW layout mismatch' }
            if ([TinHatNative]::FileInfoSize() -ne 52 -or [TinHatNative]::LinksOffset() -ne 40) { throw 'BY_HANDLE_FILE_INFORMATION layout mismatch' }
            $script:NativeReady = $true
        } catch { $script:NativeReady = $false; Write-Log "native helpers unavailable ($($_.Exception.Message)) - nothing in your folders is touched" }
    }
    return $script:NativeReady
}

# Where a path really is (junctions, links and 8.3 names resolved); '' if unknown.
function Real([string]$Path) {
    if (-not $Path -or -not (Use-Native)) { return '' }
    try { $f = [TinHatNative]::FinalPath((Norm $Path)) } catch { $f = $null }
    if (-not $f) { return '' }
    return (Norm $f)
}

# True only when the path is exactly where it says: no junction, link or short name on the
# way. Anything else is never acted on - an alias could lead somewhere that is not Tin Hat's.
function Test-Real([string]$Path) {
    $r = Real $Path
    return $r -and ($r -ieq (Norm $Path))
}

# A reparse point we must not walk into. Without the native check every reparse point
# counts (safe, only less thorough on OneDrive).
function Test-Link([System.IO.FileSystemInfo]$Item) {
    if (-not ($Item.Attributes -band [System.IO.FileAttributes]::ReparsePoint)) { return $false }
    if (Use-Native) { return [TinHatNative]::IsLink($Item.FullName) }
    return $true
}

# ---------------------------------------------------------------------------------------
# Walking and deleting without ever following a link
# ---------------------------------------------------------------------------------------

# Every file under $Dir as a path relative to it; Links counts links (to folders and to
# files) and unreadable folders met on the way (the caller then never treats the folder as
# Tin Hat's as a whole, and never copies out of it).
function Get-TreeFiles([string]$Dir) {
    $files = New-Object System.Collections.ArrayList
    $links = 0
    if (-not (Test-Real $Dir)) { return @{ Files = $files; Links = 1 } }
    $base = (Norm $Dir).TrimEnd('\').Length + 1
    $stack = New-Object System.Collections.Stack
    $stack.Push((New-Object System.IO.DirectoryInfo((Norm $Dir))))
    while ($stack.Count -gt 0) {
        $d = $stack.Pop()
        try { $entries = $d.GetFileSystemInfos() } catch { $links++; continue }
        foreach ($e in $entries) {
            if (Test-Link $e) { $links++ }
            elseif ($e -is [System.IO.DirectoryInfo]) { $stack.Push($e) }
            else { [void]$files.Add($e.FullName.Substring($base)) }
        }
    }
    return @{ Files = $files; Links = $links }
}

# A file some release shipped. (The app writes nothing next to itself: its settings, caches
# and logs live in the data folder and %LOCALAPPDATA%\TinHat whatever folder it starts from.)
function Test-OwnFile([string]$Rel) {
    return $Manifest.Contains($Rel)
}

# Permanently delete something that is Tin Hat's own (a sandboxed copy, the data folder's
# app files): a link is removed as a link, what it points to is never touched, and a path
# that is not where it says (a junction on the way) is refused.
function Remove-OwnedTree([string]$Path) {
    $p = Norm $Path
    if (-not (Test-Path -LiteralPath $p)) { return $true }
    if (-not (Test-Real $p)) { Write-Log "  $p is reached through a link - left alone"; return $false }
    return Remove-TreeSafe $p
}

function Remove-TreeSafe([string]$Path) {
    $p = Norm $Path
    try {
        if (-not (Test-Path -LiteralPath $p)) { return $true }
        $item = Get-Item -LiteralPath $p -Force -ErrorAction Stop
        if ($item.Attributes -band [System.IO.FileAttributes]::ReadOnly) {
            $item.Attributes = $item.Attributes -band (-bnot [System.IO.FileAttributes]::ReadOnly)
        }
        if ($item -is [System.IO.DirectoryInfo]) {
            if (-not (Test-Link $item)) {
                foreach ($c in $item.GetFileSystemInfos()) {
                    if (-not (Remove-TreeSafe $c.FullName)) { return $false }
                }
            }
            [System.IO.Directory]::Delete($p, $false)
        } else {
            [System.IO.File]::Delete($p)
        }
        return $true
    } catch {
        Write-Log "  could not remove $p ($($_.Exception.Message))"
        return $false
    }
}

# To the Recycle Bin, several at once. Returns the paths that are still there: Windows would
# not recycle them, so they stay (fail closed).
function Move-ToBin([string[]]$Paths) {
    $todo = @($Paths | Where-Object { $_ -and (Test-Path -LiteralPath $_) })
    if ($todo.Count -eq 0) { return @() }
    $unreal = @($todo | Where-Object { -not (Test-Real $_) })
    if ($unreal.Count -gt 0) {
        foreach ($u in $unreal) { Write-Log "  $u is reached through a link - left alone" }
        $todo = @($todo | Where-Object { Test-Real $_ })
        if ($todo.Count -eq 0) { return $unreal }
    }
    if (Use-Native) {
        # SHFileOperation takes a double-NUL list; long batches go in slices.
        for ($i = 0; $i -lt $todo.Count; $i += 200) {
            $slice = $todo[$i..([Math]::Min($i + 199, $todo.Count - 1))]
            try { $rc = [TinHatNative]::Recycle((($slice | ForEach-Object { Norm $_ }) -join "`0")) } catch { $rc = -1 }
            if ($rc -ne 0) { Write-Log "  Recycle Bin answered $rc"; $script:RecycleTrouble = $true }
        }
    } else {
        Write-Log '  Recycle Bin unavailable - nothing moved'
    }
    $left = @(@($todo | Where-Object { Test-Path -LiteralPath $_ }) + $unreal)
    # An aborted or failed batch counts as a failure even when the paths are gone.
    if ($script:RecycleTrouble) { $script:RecycleTrouble = $false; $left += '<recycle bin reported a problem>' }
    return $left
}

# Remove the folders that became empty because their Tin Hat files left, from the deepest
# up to (not including) $Stop. Folders that were empty before are not touched.
function Remove-EmptiedDirs([string[]]$Files, [string]$Stop) {
    $dirs = @{}
    foreach ($f in $Files) {
        $d = Split-Path -Parent $f
        while ($d -and (Under $d $Stop) -and -not (Same $d $Stop)) { $dirs[(Norm $d)] = $true; $d = Split-Path -Parent $d }
    }
    foreach ($d in ($dirs.Keys | Sort-Object Length -Descending)) {
        try {
            $info = New-Object System.IO.DirectoryInfo($d)
            if ($info.Exists -and -not (Test-Link $info) -and @($info.GetFileSystemInfos()).Count -eq 0) { $info.Delete() }
        } catch { }
    }
}

# ---------------------------------------------------------------------------------------
# What is Tin Hat, and where
# ---------------------------------------------------------------------------------------

function Read-Version([string]$Dir) {
    $v = ''
    try { $v = ([System.IO.File]::ReadAllText((Join-Path $Dir 'VERSION'))).Trim() } catch { }
    if ($v -notmatch '^\d+\.\d+') {
        foreach ($n in $ExeNames) {
            $exe = Join-Path $Dir $n
            if (Test-Path -LiteralPath $exe) {
                try { $v = (Get-Item -LiteralPath $exe).VersionInfo.ProductVersion } catch { }
                if ($v) { break }
            }
        }
    }
    if ($v -notmatch '^\d+\.\d+') { return '' }
    return ($v -split '[\s+]')[0]
}

# A release bundle: Qt next to the exe plus the publisher's VERSION stamp or the bundled
# GStreamer tree. A developer build under target\release has neither.
function Test-Bundle([string]$Dir) {
    if (-not (Test-Path -LiteralPath (Join-Path $Dir 'Qt6Core.dll'))) { return $false }
    return (Test-Path -LiteralPath (Join-Path $Dir 'VERSION')) -or (Test-Path -LiteralPath (Join-Path $Dir 'lib\gstreamer-1.0'))
}

# Never touch a source checkout (a developer's machine, the build VM).
function Test-DevTree([string]$Dir) {
    $d = Norm $Dir
    for ($i = 0; $i -lt 8 -and $d; $i++) {
        if ((Test-Path -LiteralPath (Join-Path $d '.git')) -or (Test-Path -LiteralPath (Join-Path $d 'Cargo.toml'))) { return $true }
        $parent = Split-Path -Parent $d
        if (-not $parent -or $parent -eq $d) { break }
        $d = $parent
    }
    return $false
}

# A folder that is never removed as a whole, even when it holds nothing but Tin Hat's files
# (a zip extracted straight into Downloads on a fresh PC): only its Tin Hat files go.
function Test-ProtectedFolder([string]$Dir) {
    $d = Norm $Dir
    $r = Real $Dir
    if (-not $r) { return $true }   # cannot tell where it really is: treat it as precious
    if ($d -match '^[A-Za-z]:\\?$' -or $d -match '^\\\\[^\\]+\\[^\\]+$') { return $true }   # drive or share root
    if ($r -match '^[A-Za-z]:\\?$' -or $r -match '^\\\\[^\\]+\\[^\\]+$') { return $true }
    foreach ($k in @($Downloads, $Desktop, $Documents, $Programs, $Startup, $env:USERPROFILE, $env:TEMP, $env:TMP,
            $env:LOCALAPPDATA, $env:APPDATA, $env:OneDrive, $env:PUBLIC, (Split-Path -Parent $env:USERPROFILE),
            [Environment]::GetFolderPath('MyPictures'), [Environment]::GetFolderPath('MyMusic'),
            [Environment]::GetFolderPath('MyVideos'), [Environment]::GetFolderPath('Windows'),
            [Environment]::GetFolderPath('ProgramFiles'), [Environment]::GetFolderPath('ProgramFilesX86'), $Root, $Current)) {
        if (-not $k) { continue }
        if (Same $d $k) { return $true }
        $kr = Real $k
        if ($kr -and ($kr -ieq $r)) { return $true }
    }
    return $false
}

# Where zip copies usually live; anything else needs a person's tick.
function Test-UsualPlace([string]$Dir) {
    return (Under $Dir $Downloads) -or (Under $Dir $Desktop) -or (Under $Dir $env:TEMP)
}

# ---------------------------------------------------------------------------------------
# close
# ---------------------------------------------------------------------------------------

# Only this session's copies (another Windows account's Tin Hat is not ours to close), and
# only processes that prove they are Tin Hat: the installed exe, a release bundle's exe,
# or a file that calls itself Tin Hat. One this token cannot even read is left alone - it
# could not be ended anyway, and the lock check reports it.
function Get-MyTinHat {
    $session = (Get-Process -Id $PID).SessionId
    $mine = @()
    foreach ($p in @(Get-Process -Name tinhat -ErrorAction SilentlyContinue)) {
        if ($p.SessionId -ne $session) { continue }
        $path = ''
        try { $path = $p.Path } catch { }
        if (-not $path) { continue }
        $ok = (Same $path $Installed) -or (Test-Bundle (Split-Path -Parent $path))
        if (-not $ok) {
            try {
                $vi = (Get-Item -LiteralPath $path).VersionInfo
                $ok = ("$($vi.ProductName) $($vi.FileDescription)" -match '(?i)tin ?hat')
            } catch { }
        }
        if ($ok) { $mine += $p }
    }
    return $mine
}

# The same process, not a new one that got its PID: id and start time must both match.
function Test-StillThere([object]$Known) {
    $p = Get-Process -Id $Known.Id -ErrorAction SilentlyContinue
    if (-not $p) { return $false }
    try { return $p.StartTime -eq $Known.Start } catch { return $false }
}

function Close-TinHat {
    $procs = @(Get-MyTinHat)
    if ($procs.Count -eq 0) { return 128 }
    $dirs = @($procs | ForEach-Object { Split-Path -Parent $_.Path })
    $known = @($procs | ForEach-Object { $st = $null; try { $st = $_.StartTime } catch { }; [pscustomobject]@{ Id = $_.Id; Start = $st } })
    Write-Log "close: Tin Hat is running ($(($procs | ForEach-Object { "$($_.Id) $($_.Path)" }) -join ', ')) - asking it to close"
    # WM_CLOSE runs the app's normal quit, which leaves a call cleanly.
    foreach ($p in $procs) { try { [void]$p.CloseMainWindow() } catch { } }
    $alive = { @($known | Where-Object { Test-StillThere $_ }) }
    $deadline = (Get-Date).AddSeconds(12)
    while (@(& $alive).Count -gt 0 -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 250 }
    $rc = 0
    $left = @(& $alive)
    if ($left.Count -gt 0) {
        # No window, not responding, or asking "leave the call?": end it (these PIDs only).
        Write-Log "close: $($left.Count) did not close by itself - ending it"
        foreach ($k in $left) {
            if (-not (Test-StillThere $k)) { continue }
            try { Stop-Process -Id $k.Id -Force -ErrorAction Stop } catch { Write-Log "close: could not end $($k.Id): $($_.Exception.Message)" }
        }
        $rc = 1
        $deadline = (Get-Date).AddSeconds(5)
        while (@(& $alive).Count -gt 0 -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 250 }
    } else {
        Write-Log 'close: Tin Hat closed by itself'
    }
    # Its web-view helper processes (QtWebEngineProcess.exe from the same folder) go with it.
    $session = (Get-Process -Id $PID).SessionId
    foreach ($h in @(Get-Process -Name QtWebEngineProcess -ErrorAction SilentlyContinue | Where-Object { $_.SessionId -eq $session })) {
        try { if ($dirs -contains (Split-Path -Parent $h.Path)) { Stop-Process -Id $h.Id -Force -ErrorAction Stop } } catch { }
    }
    if (@(& $alive).Count -gt 0) { Write-Log 'close: Tin Hat is STILL running'; return 2 }
    return $rc
}

# ---------------------------------------------------------------------------------------
# scan
# ---------------------------------------------------------------------------------------

$Findings = New-Object System.Collections.ArrayList
$SeenPaths = @{}

function Add-Finding([string]$Kind, [bool]$Ticked, [string]$Path, [string]$Label, [bool]$Auto = $true, [string]$Seen = '') {
    $key = $Kind + '|' + (Norm $Path).ToLowerInvariant()
    if ($SeenPaths.ContainsKey($key)) { return }
    $SeenPaths[$key] = $true
    $t = '0'; if ($Ticked) { $t = '1' }
    $a = '0'; if ($Auto -and $Ticked) { $a = '1' }
    [void]$Findings.Add(($Kind, $t, (Norm $Path), (Clean-Label $Label), $a, (Clean-Label $Seen)) -join "`t")
}

# A copy as it was scanned: every file's relative path, size and last-write time, hashed
# into one short string - a folder swapped or changed between the scan and the click no
# longer matches. ($OwnOnly: only the files that would be removed, for mixed folders.)
function Get-Inventory([string]$Dir, [bool]$OwnOnly = $false, [bool]$InRoot = $false) {
    $tree = Get-TreeFiles $Dir
    $lines = New-Object System.Collections.Generic.List[string]
    foreach ($rel in $tree.Files) {
        if ($OwnOnly -and -not (Test-Removable $rel $InRoot)) { continue }
        $len = -1; $mt = 0
        try { $fi = New-Object System.IO.FileInfo((Join-Path $Dir $rel)); $len = $fi.Length; $mt = $fi.LastWriteTimeUtc.Ticks } catch { }
        $lines.Add("$rel|$len|$mt")
    }
    $lines.Sort([System.StringComparer]::Ordinal)
    $tail = ''; if (-not $OwnOnly) { $tail = "|links=$($tree.Links)" }
    $bytes = [System.Text.Encoding]::UTF8.GetBytes(($lines -join "`n") + $tail)
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try { return ([System.BitConverter]::ToString($sha.ComputeHash($bytes)) -replace '-', '').Substring(0, 32) } finally { $sha.Dispose() }
}

# Every exe path Windows or the user's shortcuts remember, plus running copies.
function Get-RememberedExes {
    $paths = New-Object System.Collections.ArrayList
    $pattern = '(?i)\\tinhat\.exe'
    foreach ($key in @('HKCU:\Software\Classes\Local Settings\Software\Microsoft\Windows\Shell\MuiCache',
            'HKCU:\Software\Microsoft\Windows NT\CurrentVersion\AppCompatFlags\Compatibility Assistant\Store',
            'HKCU:\Software\Microsoft\Windows NT\CurrentVersion\AppCompatFlags\Layers')) {
        try {
            $item = Get-Item -LiteralPath $key -ErrorAction Stop
            foreach ($name in $item.GetValueNames()) {
                if ($name -match $pattern) {
                    $i = $name.IndexOf('.exe', [System.StringComparison]::OrdinalIgnoreCase)
                    [void]$paths.Add($name.Substring(0, $i + 4))
                }
            }
        } catch { }
    }
    try {
        foreach ($p in @(Get-Process -Name tinhat -ErrorAction SilentlyContinue)) {
            try { if ($p.Path) { [void]$paths.Add($p.Path) } } catch { }
        }
    } catch { }
    return $paths
}

# Bounded walk for copies somebody extracted: never follows links, never descends into
# Windows or program folders, tolerates folders it may not list.
function Find-ExeCopies([string]$Start, [int]$Depth) {
    $found = New-Object System.Collections.ArrayList
    if (-not $Start -or -not (Test-Path -LiteralPath $Start)) { return $found }
    $skip = @('AppData', 'Windows', 'Program Files', 'Program Files (x86)', 'ProgramData', '$Recycle.Bin',
        'System Volume Information', 'node_modules', '.git', 'Recovery', 'PerfLogs')
    $queue = New-Object System.Collections.Queue
    $queue.Enqueue(@($Start, 0))
    while ($queue.Count -gt 0) {
        $entry = $queue.Dequeue(); $dir = $entry[0]; $level = [int]$entry[1]
        foreach ($n in $ExeNames) {
            $exe = Join-Path $dir $n
            if ([System.IO.File]::Exists($exe)) { [void]$found.Add($exe) }
        }
        if ($level -ge $Depth) { continue }
        try { $subs = (New-Object System.IO.DirectoryInfo($dir)).GetDirectories() } catch { continue }
        foreach ($s in $subs) {
            if ($skip -contains $s.Name) { continue }
            if (Test-Link $s) { continue }
            # (Never write "$s, $level + 1": the comma binds first and the depth never grows.)
            $next = $level + 1
            $queue.Enqueue(@($s.FullName, $next))
        }
    }
    return $found
}

function Get-Shortcuts {
    $list = New-Object System.Collections.ArrayList
    $spots = @(@($Desktop, $false), @($Programs, $true), @($Startup, $false), @($TaskbarPins, $false),
        @((Join-Path $QuickLaunch 'User Pinned\StartMenu'), $false), @($QuickLaunch, $false),
        @((Join-Path $QuickLaunch 'User Pinned\ImplicitAppShortcuts'), $true))
    foreach ($spot in $spots) {
        $dir = $spot[0]
        if (-not $dir -or -not (Test-Path -LiteralPath $dir)) { continue }
        try {
            if ($spot[1]) { $items = Get-ChildItem -LiteralPath $dir -Filter *.lnk -Recurse -File -ErrorAction SilentlyContinue }
            else { $items = Get-ChildItem -LiteralPath $dir -Filter *.lnk -File -ErrorAction SilentlyContinue }
            foreach ($i in @($items)) { [void]$list.Add($i.FullName) }
        } catch { }
    }
    return $list
}

$Shell = $null
function Get-LinkTarget([string]$Lnk) {
    try {
        if (-not $script:Shell) { $script:Shell = New-Object -ComObject WScript.Shell }
        return $script:Shell.CreateShortcut($Lnk).TargetPath
    } catch { return '' }
}

# Rules live in the registry as "v2.x|Action=..|App=<path>|Name=..|" strings; reading them
# needs no administrator.
$FirewallReadFailed = $false
function Get-FirewallRules {
    $rules = New-Object System.Collections.ArrayList
    try {
        $key = Get-Item -LiteralPath 'HKLM:\SYSTEM\CurrentControlSet\Services\SharedAccess\Parameters\FirewallPolicy\FirewallRules' -ErrorAction Stop
        foreach ($id in $key.GetValueNames()) {
            $v = [string]$key.GetValue($id)
            if ($v -match '\|App=([^|]+)\|' -and ($ExeNames -contains ([System.IO.Path]::GetFileName($Matches[1])).ToLowerInvariant())) {
                [void]$rules.Add([pscustomobject]@{ Id = $id; App = [Environment]::ExpandEnvironmentVariables($Matches[1]) })
            }
        }
    } catch { $script:FirewallReadFailed = $true; Write-Log "firewall rules unreadable: $($_.Exception.Message)" }
    return $rules
}

function Test-TinHatDownload([System.IO.FileInfo]$File) {
    $n = $File.Name
    if ($n -match '^(?i)tinhat-(\d+\.\d+\.\d+)[\w.+-]*-win64( \(\d+\))?\.zip$') {
        try {
            Add-Type -AssemblyName System.IO.Compression.FileSystem -ErrorAction Stop
            $z = [System.IO.Compression.ZipFile]::OpenRead($File.FullName)
            try {
                foreach ($e in $z.Entries) {
                    if ($e.FullName -match '(^|[\\/])tinhat\.exe$') { return $true }
                }
            } finally { $z.Dispose() }
        } catch { }
        return $false
    }
    if ($n -match '^(?i)tinhat-setup(-\d+\.\d+\.\d+(-r\d+)?)?( \(\d+\))?\.exe$') {
        try {
            $vi = $File.VersionInfo
            return ("$($vi.ProductName) $($vi.FileDescription)" -match '(?i)tin ?hat')
        } catch { }
    }
    return $false
}

# A shortcut or firewall rule is Tin Hat's when it names tinhat.exe inside a release bundle
# or Tin Hat's own folder.
function Test-TinHatExe([string]$Exe) {
    $leaf = ([System.IO.Path]::GetFileName($Exe)).ToLowerInvariant()
    if ($leaf -ne 'tinhat.exe') { return $false }
    $dir = Split-Path -Parent $Exe
    return (Under $dir $Root) -or ((Test-Path -LiteralPath $dir) -and (Test-Bundle $dir))
}

function Scan {
    $candidates = @{}
    foreach ($p in (Get-RememberedExes)) { $candidates[(Norm $p).ToLowerInvariant()] = $p }
    # App files unzipped by hand straight into the data folder (next to current\, not in it).
    foreach ($n in $ExeNames) { $p = Join-Path $Root $n; $candidates[(Norm $p).ToLowerInvariant()] = $p }
    $walks = @(@($Downloads, 3), @($Desktop, 2), @($Documents, 2), @($env:USERPROFILE, 1), @($env:TEMP, 2),
        @((Join-Path $env:LOCALAPPDATA 'Programs'), 2), @(($env:SystemDrive + '\'), 1))
    if ($env:OneDrive) { $walks += , @($env:OneDrive, 2) }
    foreach ($w in $walks) {
        foreach ($p in (Find-ExeCopies $w[0] $w[1])) { $candidates[(Norm $p).ToLowerInvariant()] = $p }
    }
    # Shortcut targets are candidates too (a pin can be all that is left of a copy's path).
    $links = @(Get-Shortcuts)
    $linkTargets = @{}
    foreach ($l in $links) {
        $t = Get-LinkTarget $l
        if ($t -and (Test-TinHatExe $t)) {
            $linkTargets[$l] = $t
            $candidates[(Norm $t).ToLowerInvariant()] = $t
        }
    }
    if ($Manifest.Count -eq 0) { Write-Log 'scan: the bundle file list is missing - old copies are left alone' }

    # 1. Copies of the app outside current\.
    foreach ($exe in $candidates.Values) {
        if (-not [System.IO.File]::Exists($exe)) { continue }
        $dir = Split-Path -Parent (Norm $exe)
        # Inside the data folder only the folder itself counts (app files unzipped there by
        # hand); current\, the updater's and Setup's set-aside folders are the installer's own.
        if ((Under $dir $Root) -and -not (Same $dir $Root)) { continue }
        if (-not (Test-Bundle $dir) -or (Test-DevTree $dir)) { continue }
        try { if (Test-Link (Get-Item -LiteralPath $dir -Force)) { continue } } catch { continue }
        if (-not (Test-Real $dir)) { Write-Log "scan: $dir is reached through a link or short name - left alone"; continue }
        $ver = Read-Version $dir
        $what = 'Tin Hat'; if ($ver) { $what = "Tin Hat $ver" }
        if (Same $dir $Root) {
            Add-Finding 'rootmix' $true $dir "Take app files out of Tin Hat's data folder (they were unzipped there by hand)" $true (Get-Inventory $dir $true $true)
            continue
        }
        if ($dir -match '(?i)\\Packages\\[^\\]+\\LocalCache\\') { continue } # reported as a sandboxed copy below
        if ($Manifest.Count -eq 0) { continue }
        $tree = Get-TreeFiles $dir
        $foreign = @($tree.Files | Where-Object { -not (Test-OwnFile $_) })
        $usual = Test-UsualPlace $dir
        if ($foreign.Count -eq 0 -and $tree.Links -eq 0 -and -not (Test-ProtectedFolder $dir)) {
            Add-Finding 'copy' $true $dir "Remove old copy of $what in $(Friendly $dir)" $usual (Get-Inventory $dir)
        } else {
            # A folder that also holds other things only ever changes with a person's tick.
            $others = $foreign.Count + $tree.Links
            $s = ''; if ($others -ne 1) { $s = 's' }
            Add-Finding 'copymixed' $true $dir "Take $what's files out of $(Friendly $dir) ($others other item$s there stay)" $false (Get-Inventory $dir $true $false)
        }
    }

    # 2. Copies Windows kept inside another app's sandbox (an installer or the app started
    #    from a Store-packaged program writes to that package's private AppData instead).
    $pk = Join-Path $env:LOCALAPPDATA 'Packages'
    if (Test-Path -LiteralPath $pk) {
        foreach ($p in @(Get-ChildItem -LiteralPath $pk -Directory -ErrorAction SilentlyContinue)) {
            $hit = $false
            foreach ($sub in @('LocalCache\Local\TinHatApp', 'LocalCache\Local\TinHat', 'LocalCache\Roaming\TinHat\tinhat')) {
                if (Test-Path -LiteralPath (Join-Path $p.FullName $sub)) { $hit = $true }
            }
            if (-not $hit) { continue }
            $note = ''
            if (Test-Path -LiteralPath (Join-Path $p.FullName 'LocalCache\Local\TinHatApp\session.json')) {
                if ($Mode -eq 'repair' -and -not (Test-Path -LiteralPath (Join-Path $Root 'session.json'))) { $note = ' and move its sign-in to the installed app' }
                elseif ($Mode -eq 'repair' -or $KeepData) { $note = ' (its sign-in stays)' }
                else { $note = ' with its sign-in (signed out first)' }
            }
            Add-Finding 'vcopy' $true $p.FullName "Remove a hidden copy inside another app's sandbox ($($p.Name))$note"
        }
    }

    # 3. Downloaded zips and installers: a release name AND contents that say Tin Hat.
    foreach ($dir in @($Downloads, $Desktop)) {
        if (-not $dir -or -not (Test-Path -LiteralPath $dir)) { continue }
        foreach ($f in @(Get-ChildItem -LiteralPath $dir -File -ErrorAction SilentlyContinue)) {
            if ($Self -and (Same $f.FullName $Self)) { continue }
            if (Test-TinHatDownload $f) { Add-Finding 'download' $true $f.FullName "Remove old download $($f.Name) in $(Friendly $dir)" }
        }
    }

    # 4. Shortcuts and pins. Repair points the ones at old copies at the installed app;
    #    uninstall removes every shortcut that opens Tin Hat, wherever it lives.
    foreach ($l in $linkTargets.Keys) {
        $t = $linkTargets[$l]
        $isPin = Under $l $QuickLaunch
        $kind = 'shortcut'; if ($isPin) { $kind = 'pin' }
        $name = [System.IO.Path]::GetFileNameWithoutExtension($l)
        $where = Friendly (Split-Path -Parent $l)
        # The installer's own name in the Start menu and on the desktop ("Tin Hat") is Setup's
        # to rewrite or remove - handled once, there, with the same identity rule.
        $own = ((Same (Split-Path -Parent $l) $Programs) -or (Same (Split-Path -Parent $l) $Desktop)) -and
            ($name -ieq 'Tin Hat')
        if ($own) { continue }
        if ($Mode -eq 'uninstall') {
            if ((Test-DevTree (Split-Path -Parent $t)) -and -not (Same $t $Installed)) { continue }
            if ($isPin) { Add-Finding $kind $true $l "Unpin '$name' from the taskbar" $true $t }
            else { Add-Finding $kind $true $l "Remove shortcut '$name' ($where)" $true $t }
        } else {
            if (Same $t $Installed) { continue }
            if ([System.IO.File]::Exists($t) -and (Test-DevTree (Split-Path -Parent $t))) { continue }
            if ($isPin) { Add-Finding $kind $true $l "Point taskbar pin '$name' at the installed app (it opens an old copy)" $true $t }
            else { Add-Finding $kind $true $l "Point shortcut '$name' ($where) at the installed app (it opens an old copy)" $true $t }
        }
    }

    # 5. Windows Firewall rules Windows made for any copy (removing them needs an administrator).
    if ($Mode -eq 'uninstall') {
        $byApp = @{}
        foreach ($r in (Get-FirewallRules)) {
            if ($r.App -match '(?i)\\target\\(release|debug)\\') { continue }
            if (-not (Test-TinHatExe $r.App)) { continue }
            $byApp[(Norm $r.App).ToLowerInvariant()] = $r.App
        }
        foreach ($app in $byApp.Values) {
            Add-Finding 'firewall' $true $app "Remove the Windows Firewall rules for $(Friendly $app) (asks for administrator permission)" $false
        }
    }

    # Grouped the way the page reads: copies, downloads, shortcuts, pins, firewall.
    $order = @{ 'copy' = 0; 'copymixed' = 1; 'rootmix' = 2; 'vcopy' = 3; 'download' = 4; 'shortcut' = 5; 'pin' = 6; 'firewall' = 7 }
    $sorted = @($Findings | Sort-Object @{ Expression = { $order[($_ -split "`t")[0]] } }, @{ Expression = { ($_ -split "`t")[3] } })
    [System.IO.File]::WriteAllLines($Out, [string[]]$sorted, $Utf8Bom)
    Write-Log "scan ($Mode): $($Findings.Count) finding(s)"
    foreach ($f in $sorted) { Write-Log "  $f" }
}

# ---------------------------------------------------------------------------------------
# apply
# ---------------------------------------------------------------------------------------

# "Extract All" puts the bundle in a folder named after the zip; a zip opened straight from
# Explorer runs from %TEMP%\Temp1_<zip>. Drop that wrapper too once it is empty.
function Remove-EmptyWrapper([string]$Dir) {
    $parent = Split-Path -Parent (Norm $Dir)
    if (-not $parent -or (Test-ProtectedFolder $parent)) { return }
    $name = Split-Path -Leaf $parent
    if ($name -notmatch '(?i)^(tinhat-[\w.+-]*win64( \(\d+\))?|Temp\d*_tinhat[\w.+-]*\.zip)$') { return }
    try {
        $info = New-Object System.IO.DirectoryInfo($parent)
        if (-not (Test-Link $info) -and @($info.GetFileSystemInfos()).Count -eq 0) { $info.Delete() }
    } catch { }
}

# Only Tin Hat's own files leave a folder that also holds other things (checked again now:
# the folder may have changed since the scan). $Permanent for the app's own data folder.
# Exactly the files a folder clean-up takes out: Tin Hat's by the list, minus a user's own icon
# pack in the data folder and generic names in a folder shared with other things.
function Test-Removable([string]$Rel, [bool]$InRoot) {
    if (-not (Test-OwnFile $Rel)) { return $false }
    if ($InRoot) { return -not ($Rel -like 'iconpack\*') }
    return -not ($KeepInMixed -contains $Rel)
}

# Returns 'done', 'none' (nothing of Tin Hat's there any more) or 'failed'.
function Remove-OwnFiles([string]$Dir, [bool]$Permanent) {
    if (-not (Test-Real $Dir)) { Write-Log "  $Dir is reached through a link - left alone"; return 'failed' }
    $tree = Get-TreeFiles $Dir
    $mine = @($tree.Files | Where-Object { Test-Removable $_ $Permanent } | ForEach-Object { Join-Path $Dir $_ })
    if ($mine.Count -eq 0) { return 'none' }
    $ok = $true
    if ($Permanent) {
        foreach ($f in $mine) { if (-not (Remove-TreeSafe $f)) { $ok = $false } }
    } else {
        $left = @(Move-ToBin $mine)
        if ($left.Count -gt 0) { $ok = $false; Write-Log "  $($left.Count) file(s) could not go to the Recycle Bin and stay" }
    }
    Remove-EmptiedDirs $mine $Dir
    if ($ok) { return 'done' } else { return 'failed' }
}

# What a sandboxed copy's data folder holds that is the user's (sign-in, keys, settings)
# rather than app files, updater folders or caches.
function Get-SandboxDataEntries([string]$Data) {
    # Positively the sign-in, keys and settings names - a neighbour of the user's own (or a
    # .config holding other apps' settings) never moves.
    $names = @('session.json', 'store', 'config', 'theme', 'themes', 'themesync', 'layoutsync', 'sameroom_pairs.json',
        'nav_mode', 'tour_seen', 'space_order', 'room_recency.json', 'calendar-seen.json', 'audio_prefs.json',
        'presence-pending-off.json', 'widgets-history.json', 'mentions.json', 'dice.json', 'clocks-tz.json', 'assets',
        'avatars', 'iconpack')
    return @($names | ForEach-Object { Join-Path $Data $_ } | Where-Object { Test-Path -LiteralPath $_ } | ForEach-Object { Get-Item -LiteralPath $_ -Force })
}

# Relative path -> "length:sha256" for every file under a folder (or of one file). Case-
# exact keys: a case-sensitive folder holding both "a" and "A" must not look like one file.
function Get-Fingerprint([string]$Path) {
    $map = New-Object 'System.Collections.Generic.Dictionary[string,string]' ([System.StringComparer]::Ordinal)
    $item = Get-Item -LiteralPath $Path -Force
    if ($item -is [System.IO.FileInfo]) {
        $map[''] = "$($item.Length):$((Get-FileHash -LiteralPath $item.FullName -Algorithm SHA256).Hash)"
        return $map
    }
    $base = (Norm $Path).Length + 1
    foreach ($f in @(Get-ChildItem -LiteralPath $Path -Recurse -File -Force -ErrorAction Stop)) {
        $map[$f.FullName.Substring($base)] = "$($f.Length):$((Get-FileHash -LiteralPath $f.FullName -Algorithm SHA256).Hash)"
    }
    return $map
}

function Test-SameFingerprint([object]$A, [object]$B) {
    if ($A.Count -ne $B.Count) { return $false }
    foreach ($k in $A.Keys) { if (-not $B.ContainsKey($k) -or $B[$k] -ne $A[$k]) { return $false } }
    # Names that differ only in case cannot live side by side in the data folder.
    $folded = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::OrdinalIgnoreCase)
    foreach ($k in $A.Keys) { if (-not $folded.Add($k)) { return $false } }
    return $true
}

# A sign-in that only exists in a sandboxed copy moves to the real data folder: staged in a
# fresh folder, compared file by file (path, size, SHA-256), and only then moved into place
# - never over anything already there. On any doubt the sandbox keeps its data.
function Move-SandboxedData([string]$Data) {
    $entries = @(Get-SandboxDataEntries $Data)
    foreach ($e in $entries) {
        if (Test-Path -LiteralPath (Join-Path $Root $e.Name)) { throw "$($e.Name) already exists in $Root" }
        # Nothing is copied through a link: the entry and everything under it must be real.
        if (-not (Test-Real $e.FullName)) { throw "$($e.FullName) is reached through a link" }
        if ($e -is [System.IO.DirectoryInfo] -and (Get-TreeFiles $e.FullName).Links -gt 0) { throw "$($e.FullName) holds a link" }
    }
    New-Item -ItemType Directory -Force -Path $Root | Out-Null
    $stage = Join-Path $Root ('.migrate-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $stage | Out-Null
    $moved = @()
    try {
        foreach ($e in $entries) {
            $dest = Join-Path $stage $e.Name
            if ($e -is [System.IO.DirectoryInfo]) {
                New-Item -ItemType Directory -Path $dest | Out-Null
                foreach ($c in @(Get-ChildItem -LiteralPath $e.FullName -Force)) { Copy-Item -LiteralPath $c.FullName -Destination $dest -Recurse -Force -ErrorAction Stop }
            } else {
                Copy-Item -LiteralPath $e.FullName -Destination $dest -Force -ErrorAction Stop
            }
            if (-not (Test-SameFingerprint (Get-Fingerprint $e.FullName) (Get-Fingerprint $dest))) { throw "the copy of $($e.Name) does not match" }
        }
        foreach ($e in $entries) {
            # Exact destination, never into an existing folder (Move-Item would nest there),
            # and only what this move created is ever rolled back.
            $from = Join-Path $stage $e.Name; $to = Join-Path $Root $e.Name
            if ($e -is [System.IO.DirectoryInfo]) { [System.IO.Directory]::Move($from, $to) } else { [System.IO.File]::Move($from, $to) }
            $moved += $to
        }
    } catch {
        foreach ($m in $moved) { Remove-TreeSafe $m | Out-Null }
        throw
    } finally {
        Remove-TreeSafe $stage | Out-Null
    }
    Write-Log "  moved the sign-in found in $Data to $Root ($($entries.Count) item(s), verified)"
    return $entries
}

# A sandboxed copy: its app files and caches always go. Its data (sign-in, keys, settings)
# moves to the real folder when that has no sign-in (Repair), stays when the user keeps the
# sign-in or on any doubt, and goes (signed out first) only on a full removal.
function Remove-Sandboxed([string]$Package) {
    $data = Join-Path $Package 'LocalCache\Local\TinHatApp'
    $ok = $true
    foreach ($sub in @('current', 'backup', 'staging', 'setup-previous', 'run', 'logs', 'gstregistry.bin', 'memories-thumbs')) {
        if (-not (Remove-OwnedTree (Join-Path $data $sub))) { $ok = $false }
    }
    # The caches and Qt's local folder (TinHat\tinhat: compiled QML, the web view's cache).
    if (-not (Remove-OwnedTree (Join-Path $Package 'LocalCache\Local\TinHat'))) { $ok = $false }
    if ($Mode -eq 'uninstall' -and -not $KeepData) {
        # Only the names Tin Hat writes (the list the installer uses for the real data folder):
        # a file of the user's own next to them stays, and so does the folder then.
        foreach ($n in $KnownDataNames) {
            if (-not (Remove-OwnedTree (Join-Path $data $n))) { $ok = $false }
        }
        if (-not (Remove-OwnedTree (Join-Path $Package 'LocalCache\Roaming\TinHat\tinhat'))) { $ok = $false }
    } elseif ($Mode -eq 'repair' -and (Test-Path -LiteralPath (Join-Path $data 'session.json')) -and
              -not (Test-Path -LiteralPath (Join-Path $Root 'session.json')) -and (Test-Real $data)) {
        try {
            $moved = @(Move-SandboxedData $data)
            foreach ($e in $moved) { if (-not (Remove-OwnedTree $e.FullName)) { $ok = $false } }
        } catch {
            Write-Log "  sign-in move did not happen, the sandboxed sign-in stays: $($_.Exception.Message)"
            $ok = $false
        }
    }
    $schemes = Join-Path $Package 'LocalCache\Local\color-schemes'
    # Exactly the names the app writes: TinHatOLED, TinHatGoose, TinHat-<theme id>.
    $own = @(@('TinHatOLED.colors', 'TinHatGoose.colors') | ForEach-Object { Join-Path $schemes $_ }) +
        @(Get-ChildItem -LiteralPath $schemes -Filter 'TinHat-*.colors' -File -Force -ErrorAction SilentlyContinue | ForEach-Object { $_.FullName })
    foreach ($c in $own) {
        if (-not (Remove-OwnedTree $c)) { $ok = $false }
    }
    # Folders left empty by all that go too (never one that still holds something).
    foreach ($sub in @('LocalCache\Local\TinHatApp', 'LocalCache\Roaming\TinHat', 'LocalCache\Local\color-schemes')) {
        $d = Join-Path $Package $sub
        try {
            $info = New-Object System.IO.DirectoryInfo($d)
            if ($info.Exists -and (Test-Real $d) -and @($info.GetFileSystemInfos()).Count -eq 0) { $info.Delete() }
        } catch { }
    }
    return $ok
}

function Set-LinkTarget([string]$Lnk) {
    if (-not (Test-Real $Lnk)) { throw "$Lnk is reached through a link" }
    if (-not $script:Shell) { $script:Shell = New-Object -ComObject WScript.Shell }
    $s = $script:Shell.CreateShortcut($Lnk)
    $s.TargetPath = $Installed
    $s.Arguments = ''
    $s.WorkingDirectory = $Root
    $icon = Join-Path $Current 'tinhat.ico'
    if (Test-Path -LiteralPath $icon) { $s.IconLocation = "$icon,0" }
    $s.Save()
}

$StuckThread = $false

function Remove-Pin([string]$Lnk) {
    # Once Explorer has not answered, it is not asked again in this run.
    if ($StuckThread) { return Remove-OwnedTree $Lnk }
    # Explorer offers "Unpin from taskbar" on the pinned shortcut itself; without it the
    # taskbar would keep a dead button until Explorer restarts. The verb runs in a thread of
    # its own with a time limit: an Explorer that does not answer (hung, busy, or running in
    # another Windows session) must not hold up the removal - the pin is deleted without it.
    $ps = $null; $rs = $null
    try {
        $rs = [System.Management.Automation.Runspaces.RunspaceFactory]::CreateRunspace()
        $rs.ApartmentState = [System.Threading.ApartmentState]::STA
        $rs.Open()
        $ps = [System.Management.Automation.PowerShell]::Create()
        $ps.Runspace = $rs
        [void]$ps.AddScript({
            param($Lnk)
            $app = New-Object -ComObject Shell.Application
            $item = $app.Namespace((Split-Path -Parent $Lnk)).ParseName((Split-Path -Leaf $Lnk))
            foreach ($verb in @($item.Verbs())) {
                if (($verb.Name -replace '&', '') -match '(?i)unpin from taskbar') { $verb.DoIt(); break }
            }
        }).AddArgument($Lnk)
        $h = $ps.BeginInvoke()
        if ($h.AsyncWaitHandle.WaitOne(10000)) {
            try { [void]$ps.EndInvoke($h) } catch { }
        } else {
            Write-Log "  Explorer did not answer the unpin of $Lnk within 10 s - the pin is deleted without it"
            # A thread stuck in COM cannot be stopped; the process ends it when it exits.
            $script:StuckThread = $true
            $ps = $null; $rs = $null
        }
    } catch { }
    finally {
        if ($ps) { $ps.Dispose() }
        if ($rs) { $rs.Dispose() }
    }
    Start-Sleep -Milliseconds 300
    return Remove-OwnedTree $Lnk
}

# Does a shortcut that could not be pointed at the installed app still open the exe right in
# $Dir? (Only there: the installed app in current\ under the data folder does not count.)
function Test-StillOpened([string]$Dir, [object]$Targets) {
    $d = (Norm $Dir).TrimEnd('\')
    foreach ($t in $Targets) { if ((Split-Path -Parent (Norm $t)) -ieq $d) { return $true } }
    return $false
}

function Apply {
    if (-not $In -or -not (Test-Path -LiteralPath $In)) { throw "the list of chosen items ($In) is missing" }
    $lines = @([System.IO.File]::ReadAllLines($In, $Utf8))
    # Repair points shortcuts and pins at the installed app first; an old copy that one of them
    # still opens afterwards (it could not be re-pointed) stays, so no shortcut is left broken.
    $unmoved = New-Object System.Collections.Generic.List[string]
    if ($Mode -eq 'repair') {
        $links = @($lines | Where-Object { @('shortcut', 'pin') -contains ($_ -split "`t")[0] })
        $rest = @($lines | Where-Object { @('shortcut', 'pin') -notcontains ($_ -split "`t")[0] })
        $lines = @($links) + @($rest)
    }
    foreach ($line in $lines) {
        $f = $line -split "`t"
        if ($f.Count -lt 3) { continue }
        $kind = $f[0]; $path = $f[2]
        $seen = ''; if ($f.Count -ge 6) { $seen = $f[5] }
        $ok = $true
        $none = $false
        try {
            switch ($kind) {
                { ($_ -eq 'copy' -or $_ -eq 'copymixed' -or $_ -eq 'rootmix') -and (Test-StillOpened $path $unmoved) } {
                    Write-Log "  $path stays: a shortcut that could not be pointed at the installed app still opens it"
                    $ok = $false
                    break
                }
                'copy' {
                    # Checked again: whole-folder only if it still holds nothing but Tin Hat. If it
                    # changed since the scan it is left alone - it was offered as a whole copy.
                    $tree = Get-TreeFiles $path
                    $foreign = @($tree.Files | Where-Object { -not (Test-OwnFile $_) })
                    if ($foreign.Count -eq 0 -and $tree.Links -eq 0 -and -not (Test-ProtectedFolder $path) -and $Manifest.Count -gt 0 -and
                        (Test-Bundle $path) -and $seen -and ((Get-Inventory $path) -eq $seen)) {
                        $ok = (@(Move-ToBin @($path)).Count -eq 0)
                        if ($ok) { Remove-EmptyWrapper $path } else { Write-Log "  $path could not go to the Recycle Bin and stays" }
                    } else {
                        Write-Log "  $path changed since the scan - left alone"
                        $ok = $false
                    }
                }
                { $_ -eq 'copymixed' -or $_ -eq 'rootmix' } {
                    # Still a Tin Hat bundle, and its Tin Hat files exactly as scanned.
                    $inRoot = $kind -eq 'rootmix'
                    if (-not (Test-Bundle $path) -or -not $seen -or ((Get-Inventory $path $true $inRoot) -ne $seen)) {
                        Write-Log "  $path changed since the scan - left alone"; $ok = $false
                    } else {
                        $r = Remove-OwnFiles $path $inRoot; if ($r -eq 'none') { $none = $true }; $ok = $r -ne 'failed'
                    }
                }
                'vcopy' { $ok = Remove-Sandboxed $path }
                'download' {
                    $fi = Get-Item -LiteralPath $path -Force -ErrorAction SilentlyContinue
                    if (-not $fi -or -not ($fi -is [System.IO.FileInfo]) -or -not (Test-TinHatDownload $fi)) {
                        Write-Log "  $path is not the download it was at the scan - left alone"; $ok = $false
                    } else {
                        $ok = (@(Move-ToBin @($path)).Count -eq 0)
                        if (-not $ok) { Write-Log "  $path could not go to the Recycle Bin and stays" }
                    }
                }
                { $_ -eq 'shortcut' -or $_ -eq 'pin' } {
                    # Still pointing exactly where it did at the scan (when it was checked to open
                    # Tin Hat) - the copy it opens may already be gone by now, that is fine.
                    $t = Get-LinkTarget $path
                    if (-not $t -or -not $seen -or -not ((Norm $t) -ieq (Norm $seen))) {
                        Write-Log "  $path changed since the scan - left alone"; $ok = $false
                        if ($t) { [void]$unmoved.Add($t) }
                    } elseif ($Mode -eq 'uninstall') {
                        if ($kind -eq 'pin') { $ok = Remove-Pin $path } else { $ok = Remove-OwnedTree $path }
                    } else {
                        try { Set-LinkTarget $path } catch { [void]$unmoved.Add($t); throw }
                    }
                }
                default { continue }
            }
        } catch {
            $ok = $false
            Write-Log "  $kind $path`: $($_.Exception.Message)"
        }
        if ($none) { Write-Log "skipped: $kind $path (nothing of Tin Hat's there any more)" }
        elseif ($ok) { Write-Log "done: $kind $path" }
        else { Write-Log "FAILED: $kind $path" }
    }
}

# ---------------------------------------------------------------------------------------
# fwcheck, signout, traces
# ---------------------------------------------------------------------------------------

function Count-FirewallRules {
    if (-not $In -or -not (Test-Path -LiteralPath $In)) { Write-Log 'firewall: the list of chosen rules is missing'; return 255 }
    $apps = @([System.IO.File]::ReadAllLines($In, $Utf8) | ForEach-Object { ($_ -split "`t")[2] } | Where-Object { $_ })
    if ($apps.Count -eq 0) { Write-Log 'firewall: the list of chosen rules is empty'; return 255 }
    $n = 0
    foreach ($r in (Get-FirewallRules)) {
        foreach ($a in $apps) { if (Same $a $r.App) { $n++; break } }
    }
    if ($FirewallReadFailed) { return 255 }
    Write-Log "firewall: $n rule(s) left for the chosen apps"
    return $n
}

function Sign-OutOne([string]$File, [string]$Where) {
    if (-not (Test-Real $File)) { Write-Log "signout[$Where]: the sign-in is reached through a link - not used"; return }
    try {
        $j = [System.IO.File]::ReadAllText($File, $Utf8) | ConvertFrom-Json
        $hs = [string]$j.homeserver
        $token = ''
        if ($j.session.PSObject.Properties.Name -contains 'access_token') { $token = [string]$j.session.access_token }
        elseif ($j.session.PSObject.Properties.Name -contains 'tokens') { $token = [string]$j.session.tokens.access_token }
        if (-not $hs -or -not $token -or $hs -notmatch '^https://') { Write-Log "signout[$Where]: session.json has no usable sign-in"; return }
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        $req = [System.Net.HttpWebRequest]::Create($hs.TrimEnd('/') + '/_matrix/client/v3/logout')
        $req.Method = 'POST'
        $req.Timeout = 8000
        $req.ReadWriteTimeout = 8000
        $req.ContentType = 'application/json'
        $req.UserAgent = 'TinHat-Setup'
        $req.Headers.Add('Authorization', 'Bearer ' + $token)
        $body = [System.Text.Encoding]::ASCII.GetBytes('{}')
        $req.ContentLength = $body.Length
        $s = $req.GetRequestStream(); $s.Write($body, 0, $body.Length); $s.Close()
        try {
            $resp = $req.GetResponse(); $code = [int]$resp.StatusCode; $resp.Close()
            Write-Log "signout[$Where]: this PC signed out ($code, device $($j.session.device_id))"
        } catch [System.Net.WebException] {
            $code = 0
            if ($_.Exception.Response) { $code = [int]$_.Exception.Response.StatusCode }
            # 401 = the server no longer knows this sign-in: it is signed out already.
            if ($code -eq 401) { Write-Log "signout[$Where]: this PC signed out (it already was, 401)" }
            else { Write-Log "signout[$Where]: server not reached or refused ($code)" }
        }
    } catch {
        Write-Log "signout[$Where]: server not reached ($($_.Exception.Message))"
    }
}

# The installed app's sign-in, and those of the sandboxed copies the user left ticked in
# -In (each is its own device). An unticked copy keeps its sign-in.
function Sign-Out {
    $file = Join-Path $Root 'session.json'
    if (Test-Path -LiteralPath $file) { Sign-OutOne $file 'root' } else { Write-Log 'signout[root]: no saved sign-in' }
    if (-not $In -or -not (Test-Path -LiteralPath $In)) { return }
    foreach ($line in [System.IO.File]::ReadAllLines($In, $Utf8)) {
        $f = $line -split "`t"
        if ($f.Count -lt 3 -or $f[0] -ne 'vcopy') { continue }
        $sf = Join-Path $f[2] 'LocalCache\Local\TinHatApp\session.json'
        if (Test-Path -LiteralPath $sf) { Sign-OutOne $sf "sandbox $(Split-Path -Leaf $f[2])" }
    }
}

function Test-Meta {
    $bad = 0
    $names = @('tinhat-maint.ps1', 'tinhat-bundle-files.txt', 'run\tinhat.lock')
    if (Test-Path -LiteralPath $Root) {
        # A folder that exists but cannot be listed cannot be verified.
        try { $listing = @(Get-ChildItem -LiteralPath $Root -Force -File -ErrorAction Stop) }
        catch { Write-Log "checkmeta: cannot list $Root ($($_.Exception.Message))"; return 4 }
        foreach ($f in $listing) { if ($f.Name -match '^unins\d{3}\.(exe|dat|msg)$') { $names += $f.Name } }
    }
    foreach ($n in $names) {
        $p = Join-Path $Root $n
        if (-not (Test-Path -LiteralPath $p)) { continue }
        if (-not (Use-Native)) { Write-Log "checkmeta: cannot check $p"; return 4 }
        if (-not (Test-Real $p)) { Write-Log "checkmeta: $p is reached through a link"; $bad++; continue }
        $links = [TinHatNative]::LinkCount($p)
        if ($links -ne 1) { Write-Log "checkmeta: $p has $links names (a hard link to something else?)"; $bad++ }
    }
    if ($bad -gt 0) { return 3 }
    return 0
}

function Clear-Traces {
    $n = 0
    foreach ($key in @('HKCU:\Software\Classes\Local Settings\Software\Microsoft\Windows\Shell\MuiCache',
            'HKCU:\Software\Microsoft\Windows NT\CurrentVersion\AppCompatFlags\Compatibility Assistant\Store',
            'HKCU:\Software\Microsoft\Windows NT\CurrentVersion\AppCompatFlags\Layers')) {
        try {
            $item = Get-Item -LiteralPath $key -ErrorAction Stop
            foreach ($name in $item.GetValueNames()) {
                if ($name -notmatch '(?i)^(.+?\.exe)') { continue }
                $exe = $Matches[1]
                $ours = (Test-TinHatExe $exe) -or ([System.IO.Path]::GetFileName($exe) -match '^(?i)tinhat-setup(-[\w.]+)?( \(\d+\))?\.exe$')
                if ($ours -and $exe -notmatch '(?i)\\target\\(release|debug|win-bundle)\\') {
                    Remove-ItemProperty -LiteralPath $key -Name $name -ErrorAction SilentlyContinue
                    $n++
                }
            }
        } catch { }
    }
    # Crash reports and dumps Windows Error Reporting kept for the app.
    foreach ($spot in @((Join-Path $env:LOCALAPPDATA 'CrashDumps'), (Join-Path $env:LOCALAPPDATA 'Microsoft\Windows\WER\ReportArchive'),
            (Join-Path $env:LOCALAPPDATA 'Microsoft\Windows\WER\ReportQueue'))) {
        if (-not (Test-Path -LiteralPath $spot)) { continue }
        foreach ($i in @(Get-ChildItem -LiteralPath $spot -Force -ErrorAction SilentlyContinue)) {
            if ($i.Name -match '(?i)(^|_)tinhat\.exe[._]') { if (Remove-OwnedTree $i.FullName) { $n++ } }
        }
    }
    # Our own Setup logs (SetupLogging=yes) from earlier runs.
    foreach ($i in @(Get-ChildItem -LiteralPath $env:TEMP -Filter 'Setup Log *.txt' -File -ErrorAction SilentlyContinue)) {
        try {
            $head = (Get-Content -LiteralPath $i.FullName -TotalCount 8 -ErrorAction Stop) -join ' '
            if ($head -match '(?i)tinhat-setup' -and $i.LastWriteTime -lt (Get-Date).AddMinutes(-2)) {
                if (Remove-OwnedTree $i.FullName) { $n++ }
            }
        } catch { }
    }
    # Notifications still sitting in Action Center.
    try {
        [void][Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime]
        [Windows.UI.Notifications.ToastNotificationManager]::History.Clear($Aumid)
    } catch { }
    Write-Log "traces: $n item(s) removed"
}

try {
    if ($ManifestError) { Write-Log "bundle file list unreadable ($ManifestError) - old copies are left alone" }
    switch ($Action) {
        'close' { exit (Close-TinHat) }
        'scan' { Scan }
        'apply' { Apply }
        'fwcheck' { exit (Count-FirewallRules) }
        'signout' { Sign-Out }
        'traces' { Clear-Traces }
        'checkmeta' { exit (Test-Meta) }
    }
    # A thread left stuck in COM (see Remove-Pin) must not keep this process - and Setup - waiting.
    if ($StuckThread) { [Environment]::Exit(0) }
    exit 0
} catch {
    Write-Log "ERROR in $Action`: $($_.Exception.Message) (line $($_.InvocationInfo.ScriptLineNumber))"
    if ($StuckThread) { [Environment]::Exit(1) }
    exit 1
}
