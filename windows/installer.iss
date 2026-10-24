; Tin Hat Windows installer - per-user, NO admin. One file, three jobs (2026-09-26):
;
;   Install    lays the bundle down in the self-updater's exact layout (%LOCALAPPDATA%\
;              TinHatApp\current - UPDATE_PATH.md sec. 1) with Start-menu + desktop
;              shortcuts; the IN-APP updater owns every update from then on (a whole-folder
;              swap of current\ - which is why the uninstaller and its metadata live one
;              level UP, outside the swapped folder).
;   Repair     Install, plus a clean-up of whatever the earlier ways of getting Tin Hat
;              left behind: copies run from extracted zips (never updated; a copy left
;              running held the single-instance lock, so the installed app would not open),
;              downloaded zips, shortcuts and taskbar pins pointing at old copies, a second
;              Apps & features entry, updater leftovers, stale caches. Sign-in, keys and
;              settings stay.
;   Uninstall  removes the app and everything it put on the PC; the sign-in goes too
;              unless the user keeps it. Apps & features runs the same removal.
;
; The app's own, fixed footprint is removed by [Code] below; tinhat-maint.ps1 FINDS the
; rest (old copies, downloads, shortcuts, firewall rules, what Windows recorded).
; Silent use: /MODE=install|repair|uninstall (default install), /KEEPDATA or /REMOVEDATA
; (a silent uninstall keeps the sign-in unless /REMOVEDATA is given).
;
; Build: package.ps1 stages the bundle at target\win-bundle\current and runs
;   ISCC /DAppVer=<version> windows\installer.iss   ->  target\TinHat-Setup-<version>.exe
;
; Unsigned builds still trip SmartScreen ONCE on this exe ("Windows protected your PC"
; -> More info -> Run anyway); files the installer writes carry no Mark-of-the-Web, so the
; installed app itself launches clean. Code signing (package.ps1 TINHAT_SIGN_CMD) removes
; even that first prompt once a certificate exists.

#ifndef AppVer
#define AppVer "0.0.0"
#endif
; /DBundle=<dir>: pack an already-built bundle (an extracted release zip) - an
; installer-only respin must ship exactly the files that were tested.
#ifndef Bundle
#define Bundle "..\target\win-bundle\current"
#endif
; Every file any release shipped (the clean-up removes only these from folders that also hold
; other files). package.ps1 passes /DBundleList= a copy with this bundle's files added.
#ifndef BundleList
#define BundleList "tinhat-bundle-files.txt"
#endif
; The Microsoft Visual C++ runtime the bundle's Qt/GStreamer/KDE DLLs were built against:
; the version of the vc_redist.x64.exe windeployqt ships inside the bundle.
#define VCRedist AddBackslash(Bundle) + "vc_redist.x64.exe"
#if FileExists(VCRedist)
#define VCRuntimeVer GetVersionNumbersString(VCRedist)
#elif FileExists(AddBackslash(SourcePath) + VCRedist)
#define VCRuntimeVer GetVersionNumbersString(AddBackslash(SourcePath) + VCRedist)
#else
#define VCRuntimeVer "14.40.0.0"
#endif

[Setup]
AppId=dev.tinhat.TinHat
AppName=Tin Hat
AppVersion={#AppVer}
AppVerName=Tin Hat {#AppVer}
AppPublisher=Tin Hat
AppPublisherURL=https://tinhat.dev/
AppSupportURL=https://tinhat.dev/
AppUpdatesURL=https://tinhat.dev/
DefaultDirName={localappdata}\TinHatApp\current
UsePreviousAppDir=no
DisableDirPage=yes
DisableProgramGroupPage=yes
DisableReadyPage=yes
; The first page is the Install / Repair / Uninstall choice.
DisableWelcomePage=yes
DirExistsWarning=no
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.17763
OutputBaseFilename=TinHat-Setup-{#AppVer}
OutputDir=..\target
Compression=lzma2/fast
SolidCompression=yes
SetupIconFile=..\crates\app\tinhat.ico
; Tin Hat is a dark app (OLED / Goose); Setup and the uninstaller match it.
WizardStyle=modern dark includetitlebar
WizardSizePercent=120
WizardImageFile=installer-assets\tinhat-wizard.png
WizardImageBackColor=#000000
WizardSmallImageFile=installer-assets\tinhat-wizard-small.png
ShowLanguageDialog=no
UninstallDisplayName=Tin Hat
UninstallDisplayIcon={app}\tinhat.exe
; The updater swaps current\ wholesale; the uninstaller belongs OUTSIDE it so an
; update never deletes uninstall metadata.
UninstallFilesDir={localappdata}\TinHatApp
; "Uninstall" leaves nothing to uninstall later.
Uninstallable=not IsUninstallMode
CreateUninstallRegKey=not IsUninstallMode
; A running copy holds every DLL in current\ open. [Code] PrepareToInstall closes every
; Tin Hat first; Restart Manager (forced) is the backstop for anything else in {app}.
CloseApplications=force
CloseApplicationsFilter=*.exe,*.dll
RestartApplications=no
; Every run leaves "Setup Log <date>.txt" in %TEMP% (the uninstaller too, when started
; from Apps & features) - the first thing to ask for when something goes wrong on a
; machine we cannot see.
SetupLogging=yes
UninstallLogging=yes
; A fresh uninstall log each time: records an older installer wrote (shortcut and folder
; removals by name) must not outlive it - every removal is [Code]'s, with its checks.
UninstallLogMode=overwrite

[Messages]
FinishedHeadingLabel=Tin Hat is installed
FinishedLabel=It keeps itself up to date from now on.
FinishedLabelNoIcons=It keeps itself up to date from now on.
ConfirmUninstall=Remove Tin Hat from this PC?%n%nTin Hat, its shortcuts and caches, and old copies of Tin Hat left in Downloads, on the Desktop or in Temp are removed. Next you choose whether your sign-in stays.
UninstalledAll=Tin Hat was removed from this PC.

; (No InstallDelete / UninstallDelete sections: every deletion goes through [Code], which
; checks that each path is really where it says - no junction on the way - and that a
; shortcut by one of our names really opens Tin Hat.)

[Files]
; First in the list: ExtractTemporaryFile has to decompress everything above an entry.
; uninsneveruninstall: the uninstaller's [Code] removes these (after checking every path is
; really where it says), never Inno's log on its own.
; AfterInstall: every file is noted and re-checked before anything old is let go (Inno calls it
; even for a file the user chose to skip after a copy error).
Source: "tinhat-maint.ps1"; DestDir: "{localappdata}\TinHatApp"; Flags: ignoreversion uninsneveruninstall; Check: not IsUninstallMode; AfterInstall: NoteInstalledFile
Source: "{#BundleList}"; DestDir: "{localappdata}\TinHatApp"; DestName: "tinhat-bundle-files.txt"; Flags: ignoreversion uninsneveruninstall; Check: not IsUninstallMode; AfterInstall: NoteInstalledFile
#if FileExists(VCRedist)
; Early in the (solid) archive: Setup may have to put the runtime in place before the app files.
Source: "{#VCRedist}"; DestDir: "{app}"; Flags: ignoreversion uninsneveruninstall; Check: not IsUninstallMode; AfterInstall: NoteInstalledFile
#endif
Source: "{#Bundle}\*"; Excludes: "\vc_redist.x64.exe"; DestDir: "{app}"; Flags: recursesubdirs ignoreversion uninsneveruninstall; Check: not IsUninstallMode; AfterInstall: NoteInstalledFile

[Icons]
; uninsneveruninstall: the uninstaller's [Code] removes them only if they still open Tin Hat.
; ShortcutSlotFree: never replace a user's own "Tin Hat" shortcut (Inno would also delete a
; Tin Hat.url / .pif of the same name).
; WorkingDir: launching with CWD inside {app} (Explorer's default when Start-in is
; empty) makes the process self-lock the folder-swap updater - keep Start-in at the
; install ROOT. The app also hops CWD there itself (early_startup), belt-and-braces.
Name: "{userprograms}\Tin Hat"; Filename: "{app}\tinhat.exe"; WorkingDir: "{localappdata}\TinHatApp"; IconFilename: "{app}\tinhat.ico"; AppUserModelID: "dev.tinhat.TinHat"; Flags: uninsneveruninstall; Check: ShortcutSlotFree('programs')
Name: "{userdesktop}\Tin Hat"; Filename: "{app}\tinhat.exe"; WorkingDir: "{localappdata}\TinHatApp"; IconFilename: "{app}\tinhat.ico"; AppUserModelID: "dev.tinhat.TinHat"; Flags: uninsneveruninstall; Check: ShortcutSlotFree('desktop')

[Registry]
; A zip run that installed ITSELF (in-app "Install to your apps") registered its own
; Apps & features entry; from here on this installer owns that entry - drop the
; duplicate so the user sees exactly one Tin Hat to uninstall.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Uninstall\TinHatApp"; Flags: deletekey
; Toast attribution (the app writes the same on every start; a zip copy may have pointed
; the icon at a folder that is gone).
Root: HKCU; Subkey: "Software\Classes\AppUserModelId\dev.tinhat.TinHat"; ValueType: string; ValueName: "DisplayName"; ValueData: "Tin Hat"; Flags: uninsdeletekey; Check: not IsUninstallMode
Root: HKCU; Subkey: "Software\Classes\AppUserModelId\dev.tinhat.TinHat"; ValueType: string; ValueName: "IconUri"; ValueData: "{app}\tinhat.ico"; Flags: uninsdeletekey; Check: not IsUninstallMode


[Run]
Filename: "{app}\tinhat.exe"; WorkingDir: "{localappdata}\TinHatApp"; Description: "Open Tin Hat"; Flags: nowait postinstall skipifsilent; Check: AppReady

[Code]
// Why Setup closes Tin Hat itself (a field report: Tin Hat wouldn't open, even after
// running the installer): a running Tin Hat holds the single-instance lock
// %LOCALAPPDATA%\TinHatApp\run\tinhat.lock, opened with no sharing, and while that
// process lives every other launch - "Open Tin Hat" on the last page included - quietly
// exits. Restart Manager only closes copies that use files in {app}: a copy started from
// an extracted zip is invisible to it, and one stuck without a window never answers it.

const
  AppExe = 'tinhat.exe';
  AUMID = 'dev.tinhat.TinHat';
  // The Inno AppId's Apps and features entry, and the one the in-app "Install to your apps" adds.
  EntryKey = 'dev.tinhat.TinHat_is1';
  InAppEntryKey = 'TinHatApp';
  UninstallKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\';
  ModeInstall = 0;
  ModeRepair = 1;
  ModeUninstall = 2;

var
  Mode: Integer;
  ScannedMode: Integer;           // the mode the findings belong to; -1 = no scan yet
  KeepData: Boolean;              // Uninstall: keep sign-in, keys and settings
  ClearCaches: Boolean;           // Repair: also clear the picture/message caches
  ChoicePage, RepairPage, UninstallPage: TWizardPage;
  ScanPage: TOutputMarqueeProgressWizardPage;
  InstallButton, RepairButton, UninstallButton: TNewButton;
  RepairList, UninstallList: TNewCheckListBox;
  UninstallNote: TNewStaticText;
  Findings: TArrayOfString;       // kind TAB ticked TAB path TAB label (tinhat-maint.ps1 scan)
  RowFinding: array of Integer;   // checklist row -> index into Findings; -1 = fixed row
  DataRow, CacheRow: Integer;
  ChosenPre, ChosenPost, ChosenFirewall: TArrayOfString;
  Done, Failures: TStringList;
  MaintScript, MaintLog: String;
  InstalledVer: String;
  RepairHint: Boolean;            // Install found old copies: point at Repair when done
  SignOutState: Integer;          // 0 not tried, 1 signed out, 2 no sign-in saved, 3 server not reached
  DoneCopies, DoneRoot, DoneSandbox, DoneDownloads, DoneLinks: Integer;  // confirmed by the helper
  HelperBroke: Boolean;
  ScanFailed: Boolean;
  LockStream: TFileStream;        // Setup holds Tin Hat's single-instance lock while it works
  InstallFinished, RestartNeeded: Boolean;
  SetAsideDone: Boolean;          // THIS run renamed current\ to setup-previous\ (rollback is ours)
  InterruptedDir: String;         // a current\ of unknown completeness, kept aside until success
  RemovalDone: Boolean;           // the uninstall removal ran: run\ goes after the lock is let go
  FreshInstall: Boolean;          // no current\ before this run: a failed install removes its own
  EntryBackup, LegacyBackup, UninsBackup: TStringList;  // the entries and uninstaller files as they were
  EntryExisted, LegacyExisted, EntrySaved: Boolean;
  RegistryTouched: Boolean;       // the install step began (Inno may have rewritten the entries)
  InstalledFiles: TStringList;    // every app file Inno reported (AfterInstall), re-checked at the end
  InstallBroken: Boolean;         // an app file is missing after the install step: this run did not finish
  SlotPaths, SlotSaved: TStringList;  // the installer's own shortcut slots as they were before [Icons]
  AumidExisted: Boolean;          // the toast identity key, as it was
  AumidBackup: TStringList;
  HelperFiles, HelperSaved: TStringList;  // the helper files in the data root as they were ('' = not there)
  BrokenUninstaller: String;      // the uninstaller a run that did not finish had already written
  SlotChanged: TStringList;       // '1' for a shortcut slot [Icons] was allowed to (re)write
  RolledBack: Boolean;            // RollBack ran (it runs once)
  RuntimeMissing: Boolean;        // silent run: the Visual C++ runtime could not be put in place
  ForeignSlot: Boolean;           // a "Tin Hat" shortcut of the user's own (it opens something else) kept its slot
  GateHold: Longint;              // gate build only: a file held open to make the roll-back fail

// ---------------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------------

function CreateFileW(lpFileName: String; dwDesiredAccess, dwShareMode, lpSecurityAttributes, dwCreationDisposition,
  dwFlagsAndAttributes, hTemplateFile: Longint): Longint;
  external 'CreateFileW@kernel32.dll stdcall';
function GetFinalPathNameByHandleW(hFile: Longint; lpszFilePath: String; cchFilePath, dwFlags: Longint): Longint;
  external 'GetFinalPathNameByHandleW@kernel32.dll stdcall';
function CloseHandle(hObject: Longint): Longint;
  external 'CloseHandle@kernel32.dll stdcall';

// Where a path really is: every junction, symbolic link and 8.3 name on the way resolved;
// '' when it cannot be opened.
function RealPath(const Path: String): String;
var
  H, N: Longint;
  Buf: String;
begin
  Result := '';
  // any access, share read/write/delete, OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS (folders too)
  H := CreateFileW(Path, 0, 7, 0, 3, $02000000, 0);
  if H = -1 then
    exit;
  try
    SetLength(Buf, 1024);
    N := GetFinalPathNameByHandleW(H, Buf, 1024, 0);
    if (N > 0) and (N < 1024) then begin
      Result := Copy(Buf, 1, N);
      if Copy(Result, 1, 8) = '\\?\UNC\' then
        Result := '\\' + Copy(Result, 9, MaxInt)
      else if Copy(Result, 1, 4) = '\\?\' then
        Result := Copy(Result, 5, MaxInt);
    end;
  finally
    CloseHandle(H);
  end;
end;

// True only when the path is exactly where it says. Nothing reached through a junction,
// link or short name is ever deleted - it could lead to something that is not Tin Hat's.
function IsRealPath(const Path: String): Boolean;
var
  R: String;
begin
  R := RealPath(Path);
  Result := (R <> '') and (AnsiLowercase(RemoveBackslashUnlessRoot(R)) = AnsiLowercase(RemoveBackslashUnlessRoot(Path)));
end;

function RootDir(): String;
begin
  Result := ExpandConstant('{localappdata}\TinHatApp');
end;

function IsUninstallMode(): Boolean;
begin
  Result := Mode = ModeUninstall;
end;

function ModeName(): String;
begin
  case Mode of
    ModeRepair: Result := 'repair';
    ModeUninstall: Result := 'uninstall';
  else
    Result := 'install';
  end;
end;

function HasSwitch(const Name: String): Boolean;
var
  I: Integer;
begin
  Result := False;
  for I := 1 to ParamCount do
    if CompareText(ParamStr(I), Name) = 0 then
      Result := True;
end;

// /NAME=value from the command line; '' when absent.
function SwitchValue(const Name: String): String;
var
  I: Integer;
  P: String;
begin
  Result := '';
  for I := 1 to ParamCount do begin
    P := ParamStr(I);
    if CompareText(Copy(P, 1, Length(Name) + 1), Name + '=') = 0 then
      Result := Copy(P, Length(Name) + 2, MaxInt);
  end;
end;

function Field(const Line: String; const Index: Integer): String;
var
  Parts: TArrayOfString;
begin
  Parts := StringSplit(Line, [#9], stAll);
  if Index < GetArrayLength(Parts) then
    Result := Parts[Index]
  else
    Result := '';
end;

procedure AppendLine(var Lines: TArrayOfString; const Line: String);
var
  N: Integer;
begin
  N := GetArrayLength(Lines);
  SetArrayLength(Lines, N + 1);
  Lines[N] := Line;
end;

// "10.6.0" -> packed version; unparsable = 0.0.0.0.
function PackVer(const S: String): Int64;
var
  Parts: TArrayOfString;
  N: array[0..3] of Word;
  I: Integer;
begin
  Parts := StringSplit(S, ['.'], stAll);
  for I := 0 to 3 do begin
    N[I] := 0;
    if I < GetArrayLength(Parts) then
      N[I] := StrToIntDef(Parts[I], 0);
  end;
  Result := PackVersionComponents(N[0], N[1], N[2], N[3]);
end;

function ReadInstalledVersion(): String;
var
  S: AnsiString;
begin
  Result := '';
  if not FileExists(RootDir() + '\current\' + AppExe) then
    exit;
  if LoadStringFromFile(RootDir() + '\current\VERSION', S) then
    Result := Trim(String(S));
  if Result = '' then
    Result := 'an unknown version';
end;

function PowerShellExe(): String;
begin
  Result := ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe');
end;

function RunHidden(const Exe, Params: String): Integer;
var
  Code: Integer;
begin
  if Exec(Exe, Params, '', SW_HIDE, ewWaitUntilTerminated, Code) then
    Result := Code
  else
    Result := -1;
end;

procedure SetStatus(const S: String);
begin
  if IsUninstaller then
    UninstallProgressForm.StatusLabel.Caption := S
  else
    WizardForm.StatusLabel.Caption := S;
end;

// ---------------------------------------------------------------------------------
// The clean-up helper (tinhat-maint.ps1)
// ---------------------------------------------------------------------------------

// "FAILED: <kind> <path>" from the helper, as a line the finish page can show.
function FailureText(const S: String): String;
var
  P: Integer;
  Kind, Path: String;
begin
  P := Pos(' ', S);
  Kind := Copy(S, 1, P - 1);
  Path := Copy(S, P + 1, MaxInt);
  if (Kind = 'shortcut') or (Kind = 'pin') then
    Result := 'Could not change the shortcut ' + Path
  else
    Result := 'Could not remove ' + Path + ' (in use? Restart Windows and run Setup again, or delete it yourself)';
end;

procedure FlushMaintLog();
var
  Lines: TArrayOfString;
  I, P: Integer;
begin
  if (MaintLog = '') or not FileExists(MaintLog) then
    exit;
  if LoadStringsFromFile(MaintLog, Lines) then
    for I := 0 to GetArrayLength(Lines) - 1 do begin
      Log(Lines[I]);
      P := Pos('FAILED: ', Lines[I]);
      if P > 0 then
        Failures.Add(FailureText(Copy(Lines[I], P + 8, MaxInt)));
      P := Pos('] done: ', Lines[I]);
      if P > 0 then begin
        if (Pos('done: copy ', Lines[I]) > 0) or (Pos('done: copymixed ', Lines[I]) > 0) then DoneCopies := DoneCopies + 1
        else if Pos('done: rootmix ', Lines[I]) > 0 then DoneRoot := DoneRoot + 1
        else if Pos('done: vcopy ', Lines[I]) > 0 then DoneSandbox := DoneSandbox + 1
        else if Pos('done: download ', Lines[I]) > 0 then DoneDownloads := DoneDownloads + 1
        else DoneLinks := DoneLinks + 1;
      end;
      if (Pos('ERROR in ', Lines[I]) > 0) and not HelperBroke then begin
        HelperBroke := True;
        Failures.Add('The clean-up stopped part-way - see "Setup Log" in %TEMP% for what was left');
      end;
      if (Pos('signout[sandbox ', Lines[I]) > 0) and (Pos('server not reached', Lines[I]) > 0) then
        Failures.Add('A hidden copy''s sign-in could not be signed out on the server - remove "Tin Hat (Windows)" ' +
          'from your account''s devices on another device');
      if Pos('signout[root]: this PC signed out', Lines[I]) > 0 then
        SignOutState := 1
      else if (Pos('signout[root]: no saved sign-in', Lines[I]) > 0) or (Pos('signout[root]: session.json has no usable', Lines[I]) > 0) then
        SignOutState := 2
      else if Pos('signout[root]: ', Lines[I]) > 0 then
        SignOutState := 3;
    end;
  DeleteFile(MaintLog);
end;

function RunMaint(const Action, Extra: String): Integer;
var
  Params: String;
  Code: Integer;
begin
  Result := -1;
#ifdef GateBuild
  // Gate-only build (never shipped): the helper "cannot run", so Setup's own fallback checks
  // are what is tested.
  if GetEnv('TH3GATE_NOHELPER') = '1' then begin
    Log('gate build: the clean-up helper is disabled - skipped: ' + Action);
    exit;
  end;
#endif
  if (MaintScript = '') or not FileExists(MaintScript) then begin
    Log('clean-up helper missing - skipped: ' + Action);
    if (Action <> 'close') and not HelperBroke then begin
      HelperBroke := True;
      Failures.Add('Setup''s clean-up helper could not run: old copies, downloads and shortcuts were not checked' +
        ' and this PC was not signed out');
    end;
    exit;
  end;
  Params := '-NoProfile -NonInteractive -ExecutionPolicy Bypass -File "' + MaintScript + '" -Action ' + Action +
    ' -Mode ' + ModeName() + ' -Log "' + MaintLog + '" ' + Extra;
  if KeepData and (Mode = ModeUninstall) then
    Params := Params + ' -KeepData';
  if Exec(PowerShellExe(), Params, '', SW_HIDE, ewWaitUntilTerminated, Code) then
    Result := Code
  else begin
    Log(Format('could not start PowerShell for %s (%s)', [Action, SysErrorMessage(Code)]));
    if (Action <> 'close') and not HelperBroke then begin
      HelperBroke := True;
      Failures.Add('Setup''s clean-up helper could not run (PowerShell did not start): old copies, downloads and' +
        ' shortcuts were not checked and this PC was not signed out');
    end;
  end;
  if (Result <> 0) and (Action <> 'close') and (Action <> 'fwcheck') and (Action <> 'checkmeta') then begin
    Log(Format('clean-up helper %s ended with %d', [Action, Result]));
    if (Action <> 'scan') and not HelperBroke then begin
      HelperBroke := True;
      Failures.Add('The clean-up stopped part-way - see "Setup Log" in %TEMP% for what was left');
    end;
  end;
  FlushMaintLog();
end;

procedure PrepareMaint();
begin
  if MaintScript <> '' then
    exit;
  MaintLog := ExpandConstant('{tmp}\tinhat-maint.log');
  if IsUninstaller then begin
    // The uninstaller runs the copy Setup left in the data root (it goes with the root).
    MaintScript := ExpandConstant('{tmp}\tinhat-maint.ps1');
    FileCopy(RootDir() + '\tinhat-bundle-files.txt', ExpandConstant('{tmp}\tinhat-bundle-files.txt'), False);
    if not FileCopy(RootDir() + '\tinhat-maint.ps1', MaintScript, False) then
      MaintScript := '';
  end else begin
    ExtractTemporaryFile('tinhat-maint.ps1');
    ExtractTemporaryFile('tinhat-bundle-files.txt');
    MaintScript := ExpandConstant('{tmp}\tinhat-maint.ps1');
  end;
end;

function ApplyList(const Items: TArrayOfString; const Name: String): Integer;
var
  F: String;
begin
  Result := 0;
  if GetArrayLength(Items) = 0 then
    exit;
  F := ExpandConstant('{tmp}\') + Name;
  if not SaveStringsToUTF8File(F, Items, False) then begin
    Log('could not write ' + F);
    Failures.Add('Setup could not hand the chosen items to its clean-up helper (see "Setup Log" in %TEMP%)');
    Result := -1;
    exit;
  end;
  Result := RunMaint('apply', '-In "' + F + '"');
end;

procedure Scan(const ShowPage: Boolean);
var
  OutFile: String;
  Rc: Integer;
begin
  Rc := -1;
  if ScannedMode = Mode then
    exit;
  PrepareMaint();
  OutFile := ExpandConstant('{tmp}\tinhat-scan.txt');
  DeleteFile(OutFile);
  if ShowPage then begin
    ScanPage.SetText('Looking for old copies of Tin Hat, downloads and shortcuts on this PC...', '');
    ScanPage.Show;
  end;
  try
    if ShowPage then
      ScanPage.Animate;
    // ({srcexe} does not exist in the uninstaller.)
    if IsUninstaller then
      Rc := RunMaint('scan', '-Out "' + OutFile + '"')
    else
      Rc := RunMaint('scan', '-Out "' + OutFile + '" -Self "' + ExpandConstant('{srcexe}') + '"');
  finally
    if ShowPage then
      ScanPage.Hide;
  end;
  SetArrayLength(Findings, 0);
  if (Rc = 0) and FileExists(OutFile) and LoadStringsFromFile(OutFile, Findings) then
    ScannedMode := Mode
  else if not ScanFailed then begin
    ScanFailed := True;
    Failures.Add('Setup could not check this PC for old copies, downloads and shortcuts (see "Setup Log" in %TEMP%)');
  end;
end;

// Nobody reviews the list in silent runs and in the Apps & features uninstaller: act only
// on findings the helper marks "auto" (the usual places a zip copy lives, the app's own
// spots) - never on a copy somewhere unusual, never on firewall rules (administrator).
procedure ChooseDefaults();
var
  I: Integer;
  Kind: String;
begin
  SetArrayLength(ChosenPre, 0);
  SetArrayLength(ChosenPost, 0);
  SetArrayLength(ChosenFirewall, 0);
  for I := 0 to GetArrayLength(Findings) - 1 do begin
    Kind := Field(Findings[I], 0);
    if (Field(Findings[I], 4) <> '1') or (Kind = 'firewall') then
      continue;
    // Repair acts only once the new app is in place (ssPostInstall); Uninstall right away.
    if Mode = ModeRepair then
      AppendLine(ChosenPost, Findings[I])
    else
      AppendLine(ChosenPre, Findings[I]);
  end;
end;

// ---------------------------------------------------------------------------------
// Closing Tin Hat
// ---------------------------------------------------------------------------------

function LockPath(): String;
begin
  Result := RootDir() + '\run\tinhat.lock';
end;

// The fallback when PowerShell cannot run the helper: taskkill, limited to this user.
// The wait runs in a child process, so Setup keeps its window painted meanwhile.
procedure WaitForTinHatExit(Seconds: Integer);
begin
  RunHidden(PowerShellExe(),
    '-NoProfile -NonInteractive -Command "$t = (Get-Date).AddSeconds(' + IntToStr(Seconds) + '); ' +
    '$s = (Get-Process -Id $PID).SessionId; ' +
    'while ((Get-Process tinhat -ErrorAction SilentlyContinue | Where-Object { $_.SessionId -eq $s }) -and ((Get-Date) -lt $t)) ' +
    '{ Start-Sleep -Milliseconds 250 }"');
end;

function CloseWithTaskkill(): Boolean;
var
  Taskkill, Who: String;
  Rc: Integer;
begin
  Taskkill := ExpandConstant('{sys}\taskkill.exe');
  Who := '/FI "USERNAME eq ' + GetUserNameString() + '" ';
  // taskkill exits with 128 when no such process exists.
  Rc := RunHidden(Taskkill, Who + '/IM ' + AppExe);
  Result := Rc <> 128;
  if not Result then
    exit;
  Log(Format('Tin Hat is running - asked it to close (taskkill exit %d)', [Rc]));
  WaitForTinHatExit(12);
  Rc := RunHidden(Taskkill, '/F ' + Who + '/IM ' + AppExe);
  Log(Format('taskkill /F exit %d', [Rc]));
  WaitForTinHatExit(5);
end;

// Hold Tin Hat's single-instance lock ourselves while files change: a Tin Hat started in
// the meantime finds it taken and quits instead of loading half-replaced files.
function TakeLock(): Boolean;
begin
  Result := LockStream <> nil;
  if Result then
    exit;
  ForceDirectories(RootDir() + '\run');
  if not IsRealPath(RootDir() + '\run') then begin
    Log(RootDir() + '\run is reached through a link - not taking the lock there');
    exit;
  end;
  try
    // Exclusive (no sharing) and never truncating: an existing lock file is opened as it
    // is (it could be a hard link to something else), a missing one is created.
    if FileExists(LockPath()) then begin
      if not IsRealPath(LockPath()) then begin
        Log(LockPath() + ' is reached through a link - not using it');
        exit;
      end;
      LockStream := TFileStream.Create(LockPath(), fmOpenReadWrite or fmShareExclusive);
    end else
      LockStream := TFileStream.Create(LockPath(), fmCreate or fmShareExclusive);
    Result := True;
  except
    LockStream := nil;
  end;
end;

// Taking the lock IS the check: nobody else can hold it while we do. A dead process's
// handles close a beat later, and an antivirus may still be peeking - a few tries.
function TakeLockWithRetries(): Boolean;
var
  I: Integer;
begin
  for I := 1 to 10 do begin
    Result := TakeLock();
    if Result then
      exit;
    Sleep(300);
  end;
end;

procedure ReleaseLock();
begin
  if LockStream <> nil then begin
    LockStream.Free;
    LockStream := nil;
  end;
end;

// Every copy this user has running, wherever it runs from (an extracted zip included).
// Returns True when one was running.
function CloseTinHat(): Boolean;
var
  Rc: Integer;
begin
  PrepareMaint();
  Rc := RunMaint('close', '');
  if Rc = -1 then
    Result := CloseWithTaskkill()
  else
    Result := Rc <> 128;
end;

// ---------------------------------------------------------------------------------
// The app's own footprint
// ---------------------------------------------------------------------------------

procedure LinkedFailure(const Path: String);
begin
  Log(Path + ' is reached through a link - left alone');
  Failures.Add(Path + ' was left alone: it leads somewhere else (a junction or link)');
end;

procedure DeleteTree(const Path: String);
begin
  if not DirExists(Path) then
    exit;
  if not IsRealPath(Path) then begin
    LinkedFailure(Path);
    exit;
  end;
  if not DelTree(Path, True, True, True) then begin
    Log('could not fully remove ' + Path);
    Failures.Add('Could not remove ' + Path + ' (in use? Restart Windows and run Setup again)');
  end;
end;

procedure DeleteOneFile(const Path: String);
begin
  if not FileExists(Path) then
    exit;
  if not IsRealPath(Path) then begin
    LinkedFailure(Path);
    exit;
  end;
  if not DeleteFile(Path) then begin
    Log('could not remove ' + Path);
    Failures.Add('Could not remove ' + Path + ' (in use? Restart Windows and run Setup again)');
  end;
end;

procedure RemoveIfEmpty(const Path: String);
begin
  if DirExists(Path) then
    RemoveDir(Path);
end;

// tinhat.exe inside Tin Hat's folder or a release bundle - the rule tinhat-maint.ps1 uses
// (Test-TinHatExe).
function IsTinHatExe(const Exe: String): Boolean;
var
  Leaf, Dir: String;
begin
  Leaf := Lowercase(ExtractFileName(Exe));
  Dir := ExtractFilePath(Exe);
  Result := (Leaf = 'tinhat.exe') and ((Pos(AnsiLowercase(AddBackslash(RootDir())), AnsiLowercase(Dir)) = 1) or
    (FileExists(Dir + 'Qt6Core.dll') and (FileExists(Dir + 'VERSION') or DirExists(Dir + 'lib\gstreamer-1.0'))));
end;

// Where a shortcut points ('' if it cannot be read).
function ShortcutTarget(const Lnk: String): String;
var
  Shell, Link: Variant;
begin
  Result := '';
  try
    Shell := CreateOleObject('WScript.Shell');
    Link := Shell.CreateShortcut(Lnk);
    Result := Link.TargetPath;
  except
    Log('could not read shortcut ' + Lnk + ': ' + GetExceptionMessage);
  end;
end;

// A shortcut by our name ("Tin Hat") is Tin Hat's when it opens Tin Hat. A target that is
// there must prove it (IsTinHatExe); a target that is gone - an old copy already removed -
// counts by its name (tinhat.exe). A user's own shortcut of that name to anything else stays.
function ShortcutOpensTinHat(const Lnk: String): Boolean;
var
  Shell, Link: Variant;
  Target, Leaf: String;
begin
  Result := False;
  try
    Shell := CreateOleObject('WScript.Shell');
    Link := Shell.CreateShortcut(Lnk);
    Target := Link.TargetPath;
    Leaf := Lowercase(ExtractFileName(Target));
    if Leaf = 'tinhat.exe' then
      Result := (not FileExists(Target)) or IsTinHatExe(Target);
  except
    Log('could not read shortcut ' + Lnk + ': ' + GetExceptionMessage);
  end;
end;

// The [Icons] Check: install our shortcut only where it replaces nothing of the user's.
function ShortcutSlotFree(const Where: String): Boolean;
var
  Base: String;
  Slot: Integer;
begin
  Result := False;
  if IsUninstallMode then
    exit;
  if Where = 'desktop' then
    Base := ExpandConstant('{userdesktop}\Tin Hat')
  else
    Base := ExpandConstant('{userprograms}\Tin Hat');
  // (Inno also clears an extensionless "Tin Hat" folder shortcut of that name.)
  Result := not FileExists(Base + '.url') and not FileExists(Base + '.pif') and not DirExists(Base) and
    ((not FileExists(Base + '.lnk')) or ShortcutOpensTinHat(Base + '.lnk'));
  if Where = 'desktop' then Slot := 1 else Slot := 0;
  // One that could not be copied first is never replaced: a failed run could not put it back.
  if Result and FileExists(Base + '.lnk') and (SlotSaved <> nil) and (SlotSaved.Count > Slot) and (SlotSaved[Slot] = '?') then begin
    Log('could not keep a copy of ' + Base + '.lnk first - left as it is');
    Result := False;
    exit;
  end;
  if not Result then begin
    Log('left the user''s own ' + Base + ' shortcut alone');
    ForeignSlot := True;
  end
  else if FileExists(Base + '.lnk') then
    Log('replacing ' + Base + '.lnk (it opened Tin Hat)');
  if Result and (SlotChanged <> nil) and (SlotChanged.Count > Slot) then
    SlotChanged[Slot] := '1';
end;

procedure DeleteShortcutIfOurs(const Lnk: String);
begin
  if FileExists(Lnk) and ShortcutOpensTinHat(Lnk) then begin
    Log('removing ' + Lnk + ' (it opened Tin Hat)');
    DeleteOneFile(Lnk);
  end;
end;

procedure RemoveShortcuts();
var
  Spots: TArrayOfString;
  I: Integer;
begin
  SetArrayLength(Spots, 5);
  Spots[0] := ExpandConstant('{userprograms}');
  Spots[1] := ExpandConstant('{userdesktop}');
  Spots[2] := ExpandConstant('{userstartup}');
  Spots[3] := ExpandConstant('{userappdata}\Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar');
  Spots[4] := ExpandConstant('{userappdata}\Microsoft\Internet Explorer\Quick Launch\User Pinned\StartMenu');
  for I := 0 to GetArrayLength(Spots) - 1 do
    DeleteShortcutIfOurs(Spots[I] + '\Tin Hat.lnk');
end;

// Everything under the data root except the uninstaller's own files, which the running
// uninstaller removes (with the then-empty folder) as its last step.
// Everything Tin Hat itself writes in its data folder (crates/app: app_data_dir users, the
// updater, the installer). Only these go - a file of the user's own in there stays, and so
// does the folder then.
procedure DeleteKnownRootData();
var
  Names: TArrayOfString;
  I: Integer;
  R, P: String;
begin
  R := RootDir();
  // (The same list as tinhat-maint.ps1 $KnownDataNames; 'run' goes last, after the lock is let go.)
  Names := StringSplit('session.json|store|config|theme|themes|themesync|layoutsync|sameroom_pairs.json|nav_mode|' +
    'tour_seen|space_order|room_recency.json|calendar-seen.json|audio_prefs.json|presence-pending-off.json|' +
    'widgets-history.json|mentions.json|dice.json|clocks-tz.json|memories-thumbs|assets|avatars|iconpack|logs|' +
    'gstregistry.bin|install-prompt-seen|tinhat-maint.ps1|tinhat-bundle-files.txt|current|backup|staging|' +
    'setup-previous|setup-interrupted|setup-interrupted-1|setup-interrupted-2|setup-interrupted-3', ['|'], stExcludeEmpty);
  for I := 0 to GetArrayLength(Names) - 1 do begin
    P := R + '\' + Names[I];
    if DirExists(P) then
      DeleteTree(P)
    else
      DeleteOneFile(P);
  end;
end;

// Tin Hat's KDE colour schemes, exactly the names the app writes (colorscheme.rs): TinHatOLED,
// TinHatGoose and TinHat-<theme id>. Each file is checked to be where it says.
procedure DeleteColourSchemes();
var
  Dir: String;
  FindRec: TFindRec;
  Found: TStringList;
  I: Integer;
begin
  Dir := ExpandConstant('{localappdata}\color-schemes');
  if not DirExists(Dir) then
    exit;
  Found := TStringList.Create;
  try
    Found.Add(Dir + '\TinHatOLED.colors');
    Found.Add(Dir + '\TinHatGoose.colors');
    // (Listed by pattern, then each file deleted on its own after the checks.)
    if FindFirst(Dir + '\TinHat-*.colors', FindRec) then begin
      try
        repeat
          if FindRec.Attributes and FILE_ATTRIBUTE_DIRECTORY = 0 then
            Found.Add(Dir + '\' + FindRec.Name);
        until not FindNext(FindRec);
      finally
        FindClose(FindRec);
      end;
    end;
    for I := 0 to Found.Count - 1 do
      DeleteOneFile(Found[I]);
  finally
    Found.Free;
  end;
  RemoveIfEmpty(Dir);
end;

// The program of a command line: "C:\path with spaces\unins000.exe" /LOG -> the path.
function FirstToken(const CmdLine: String): String;
var
  T: String;
  P: Integer;
begin
  T := Trim(CmdLine);
  if Copy(T, 1, 1) = '"' then begin
    Delete(T, 1, 1);
    P := Pos('"', T);
    if P > 0 then
      Result := Copy(T, 1, P - 1)
    else
      Result := T;
  end else begin
    P := Pos(' ', T);
    if P > 0 then
      Result := Copy(T, 1, P - 1)
    else
      Result := T;
  end;
end;

// What Tin Hat itself writes: the bundle and its updater folders, caches inside and
// outside its folder, Qt's caches, the colour schemes, shortcuts, and its registry keys.
procedure RemoveFootprint(const KeepSignIn: Boolean);
var
  R, UninsExe: String;
begin
  R := RootDir();
  SetStatus('Removing Tin Hat...');
  DeleteTree(R + '\current');
  DeleteTree(R + '\setup-previous');
  DeleteTree(R + '\backup');
  DeleteTree(R + '\staging');
  DeleteTree(R + '\logs');
  DeleteOneFile(R + '\gstregistry.bin');
  DeleteOneFile(R + '\install-prompt-seen');
  DeleteOneFile(R + '\tinhat-maint.ps1');
  DeleteOneFile(R + '\tinhat-bundle-files.txt');
  DeleteOneFile(R + '\config\tinhat\boot-state');

  SetStatus('Removing caches...');
  // The Rust caches (timeline, members, media, avatars, link previews) and Qt's local
  // folder (TinHat\tinhat: compiled QML, the web view's cache) - caches only.
  DeleteTree(ExpandConstant('{localappdata}\TinHat'));
  DeleteTree(AddBackslash(GetTempDir()) + 'tinhat-mpv-stub');
  DeleteColourSchemes();

  SetStatus('Removing shortcuts and registry entries...');
  RemoveShortcuts();
  RegDeleteKeyIncludingSubkeys(HKCU, 'Software\Classes\AppUserModelId\' + AUMID);
  RegDeleteKeyIncludingSubkeys(HKCU, UninstallKey + InAppEntryKey);

  if not IsUninstaller then begin
    // Setup's own "Uninstall": the Apps & features entry and its uninstaller go too.
    // (exactly the files the entry names - never a wildcard or a guess: Inno picks
    // unins001 when a file of someone else's already holds unins000)
    if RegQueryStringValue(HKCU, UninstallKey + EntryKey, 'UninstallString', UninsExe) then begin
      UninsExe := FirstToken(UninsExe);
      if (CompareText(ExtractFilePath(UninsExe), AddBackslash(R)) = 0) and
         (CompareText(Copy(ExtractFileName(UninsExe), 1, 5), 'unins') = 0) and (CompareText(ExtractFileExt(UninsExe), '.exe') = 0) then begin
        DeleteOneFile(UninsExe);
        DeleteOneFile(ChangeFileExt(UninsExe, '.dat'));
        DeleteOneFile(ChangeFileExt(UninsExe, '.msg'));
      end;
    end;
    RegDeleteKeyIncludingSubkeys(HKCU, UninstallKey + EntryKey);
  end;

  if not KeepSignIn then begin
    SetStatus('Removing your sign-in and settings...');
    RegDeleteKeyIncludingSubkeys(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Notifications\Settings\' + AUMID);
    // Qt's settings and web-view profile (organization TinHat, application tinhat); only
    // Tin Hat's part. (Its local folder went with the caches above.)
    RegDeleteKeyIncludingSubkeys(HKCU, 'Software\TinHat\tinhat');
    RegDeleteKeyIfEmpty(HKCU, 'Software\TinHat');
    DeleteTree(ExpandConstant('{userappdata}\TinHat\tinhat'));
    RemoveIfEmpty(ExpandConstant('{userappdata}\TinHat'));
    DeleteOneFile(ExpandConstant('{%USERPROFILE}\tinhat-diagnostics.txt'));
    DeleteKnownRootData();
  end;
  // (The lock stays held until the very end - usPostUninstall / DeinitializeSetup - and its
  // folder goes after that.)
  RemovalDone := True;
  if not IsUninstaller then
    RemoveIfEmpty(R);
end;

// Repair keeps the data but drops what can go stale: the watch-party placeholder in %TEMP%
// and (ticked by default) the picture/message caches.
procedure RepairFootprint();
begin
  DeleteTree(AddBackslash(GetTempDir()) + 'tinhat-mpv-stub');
  if ClearCaches then begin
    DeleteTree(ExpandConstant('{localappdata}\TinHat'));
    Done.Add('Cleared the picture and message caches');
  end;
end;

// ---------------------------------------------------------------------------------
// Microsoft Visual C++ runtime (Qt, GStreamer and the KDE libraries load it from System32)
// ---------------------------------------------------------------------------------

// The lowest version of the three runtime DLLs Qt, GStreamer and KDE load; '' if one is missing.
function VCRuntimeFound(): String;
var
  Names: TArrayOfString;
  V: String;
  I: Integer;
begin
  Result := '';
  SetArrayLength(Names, 3);
  Names[0] := 'msvcp140.dll';
  Names[1] := 'vcruntime140.dll';
  Names[2] := 'vcruntime140_1.dll';
  for I := 0 to 2 do begin
    if not GetVersionNumbersString(ExpandConstant('{sys}\') + Names[I], V) then begin
      Result := '';
      exit;
    end;
    if (Result = '') or (ComparePackedVersion(PackVer(V), PackVer(Result)) < 0) then
      Result := V;
  end;
end;

function VCRuntimeOK(): Boolean;
var
  Have: String;
begin
#ifdef GateBuild
  if GetEnv('TH3GATE_NORUNTIME') = '1' then begin
    Log('gate build: pretending the Visual C++ runtime is missing');
    Result := False;
    exit;
  end;
#endif
  Have := VCRuntimeFound();
  Result := (Have <> '') and (ComparePackedVersion(PackVer(Have), PackVer('{#VCRuntimeVer}')) >= 0);
end;

// Asks before the Visual C++ runtime is put in place (Windows then asks for permission).
function AskForRuntime(const StopText: String): Boolean;
begin
  Result := TaskDialogMsgBox('Tin Hat needs a part of Windows that is missing',
    'The Microsoft Visual C++ runtime on this PC is missing or too old, and Tin Hat cannot start without it. ' +
    'Setup can install it now; Windows will ask for administrator permission.' + StopText,
    mbInformation, MB_OKCANCEL, ['Install it now', 'Not now'], IDOK) = IDOK;
end;

// Runs the runtime's own installer. True = the runtime is there now (or after a restart).
function RunVCRedist(const Redist: String): Boolean;
var
  Code: Integer;
begin
  if not ShellExec('', Redist, '/install /passive /norestart', '', SW_SHOW, ewWaitUntilTerminated, Code) then
    Code := -1;
  Log(Format('vc_redist exit %d', [Code]));
  // 0 = installed, 3010 = installed but Windows must restart first, 1638 = a newer one is there.
  Result := True;
  if Code = 3010 then begin
    RestartNeeded := True;
    Done.Add('Installed the Microsoft Visual C++ runtime Tin Hat needs - restart Windows before opening Tin Hat');
  end else if VCRuntimeOK() then
    Done.Add('Installed the Microsoft Visual C++ runtime Tin Hat needs')
  else
    Result := False;
end;

// Interactive runs, before anything changes: the runtime first. False = Setup stops (the user
// said "Not now", or it did not install).
function PutRuntimeInPlaceFirst(var Why: String): Boolean;
var
  Redist: String;
begin
  Result := True;
  if WizardSilent or VCRuntimeOK() then
    exit;
  Log('Visual C++ runtime ' + VCRuntimeFound() + ' missing or older than {#VCRuntimeVer} - asking first');
  if not AskForRuntime(' Without it Setup stops here, with nothing changed.') then begin
    Why := 'Tin Hat needs the Microsoft Visual C++ runtime, which is missing or too old on this PC, so Setup stopped ' +
      'without changing anything. Run this installer again and choose "Install it now".';
    Result := False;
    exit;
  end;
  try
    ExtractTemporaryFile('vc_redist.x64.exe');
    Redist := ExpandConstant('{tmp}\vc_redist.x64.exe');
  except
    Redist := '';
  end;
  if (Redist = '') or not RunVCRedist(Redist) then begin
    Why := 'The Microsoft Visual C++ runtime did not install, so Setup stopped without changing anything else. ' +
      'Restart Windows, then run this installer again.';
    Result := False;
  end;
end;

// After the app files: the runtime is checked again (a silent run could not ask before). Silent
// runs cannot put it in place - they only say so, and old copies stay (RuntimeMissing).
procedure EnsureVCRuntime();
var
  Have, Redist: String;
begin
  Have := VCRuntimeFound();
  if VCRuntimeOK() then begin
    Log('Visual C++ runtime ' + Have + ' present (needs {#VCRuntimeVer})');
    exit;
  end;
  if RestartNeeded then begin
    // Installed before the app files; Windows puts it in place at the restart.
    Log('Visual C++ runtime installed - it is in place after Windows restarts');
    exit;
  end;
  Redist := ExpandConstant('{app}\vc_redist.x64.exe');
  if Have = '' then
    Log('Visual C++ runtime missing (needs {#VCRuntimeVer})')
  else
    Log('Visual C++ runtime ' + Have + ' is older than {#VCRuntimeVer}');
  if WizardSilent or not FileExists(Redist) then begin
    Failures.Add('The Microsoft Visual C++ runtime is missing or old - Tin Hat may not start. Install "Visual C++ Redistributable x64" from Microsoft.');
    exit;
  end;
  if not AskForRuntime('') then begin
    Failures.Add('The Microsoft Visual C++ runtime was not installed - Tin Hat may not start until it is.');
    exit;
  end;
  SetStatus('Installing the Microsoft Visual C++ runtime...');
  if not RunVCRedist(Redist) then
    Failures.Add('The Microsoft Visual C++ runtime did not install - Tin Hat may not start until it is.');
end;

// ---------------------------------------------------------------------------------
// Wizard: the choice page, the Repair and Uninstall checklists, the finish text
// ---------------------------------------------------------------------------------

function AddRow(List: TNewCheckListBox; const Caption: String; const Kind, Finding: Integer; const Checked: Boolean): Integer;
begin
  // Kind: 0 = group heading, 1 = fixed (always done), 2 = the user's choice, 3 = a note
  case Kind of
    0: begin
         Result := List.AddGroup(Caption, '', 0, nil);
         List.ItemFontStyle[Result] := [fsBold];
       end;
    1: Result := List.AddCheckBox(Caption, '', 1, True, False, False, False, nil);
    3: begin
         Result := List.AddGroup(Caption, '', 1, nil);
         List.ItemFontStyle[Result] := [];
       end;
  else
    Result := List.AddCheckBox(Caption, '', 1, Checked, True, False, False, nil);
  end;
  SetArrayLength(RowFinding, Result + 1);
  RowFinding[Result] := Finding;
end;

procedure AddFindings(List: TNewCheckListBox; const Heading, Nothing: String);
var
  I, Rows: Integer;
begin
  AddRow(List, Heading, 0, -1, False);
  Rows := 0;
  for I := 0 to GetArrayLength(Findings) - 1 do begin
    if Field(Findings[I], 3) = '' then
      continue;
    AddRow(List, Field(Findings[I], 3), 2, I, Field(Findings[I], 1) = '1');
    Rows := Rows + 1;
  end;
  if Rows = 0 then
    AddRow(List, Nothing, 3, -1, False);
end;

procedure FillRepairList();
begin
  RepairList.Items.Clear;
  SetArrayLength(RowFinding, 0);
  AddRow(RepairList, 'Always done', 0, -1, False);
  AddRow(RepairList, 'Close every copy of Tin Hat that is running or stuck', 1, -1, True);
  AddRow(RepairList, 'Put back every app file (Tin Hat {#AppVer})', 1, -1, True);
  AddRow(RepairList, 'Recreate the Start menu and desktop shortcuts and the Apps and features entry', 1, -1, True);
  AddRow(RepairList, 'Reset the start-up, plugin and graphics caches', 1, -1, True);
  AddFindings(RepairList, 'Found on this PC', 'Nothing else needs fixing: no old copies, downloads or broken shortcuts');
  AddRow(RepairList, 'Also', 0, -1, False);
  CacheRow := AddRow(RepairList, 'Clear the picture and message caches (they refill by themselves)', 2, -1, True);
  DataRow := -1;
end;

procedure FillUninstallList();
begin
  UninstallList.Items.Clear;
  SetArrayLength(RowFinding, 0);
  AddRow(UninstallList, 'Always removed', 0, -1, False);
  AddRow(UninstallList, 'Tin Hat itself and its Start menu, desktop and taskbar shortcuts', 1, -1, True);
  AddRow(UninstallList, 'Its Apps and features entry and its Windows notification registration', 1, -1, True);
  AddRow(UninstallList, 'Its caches, logs and colour schemes', 1, -1, True);
  AddRow(UninstallList, 'Your data', 0, -1, False);
  DataRow := AddRow(UninstallList, 'Your sign-in, keys and settings on this PC (this PC is signed out first)', 2, -1, not KeepData);
  AddFindings(UninstallList, 'Also found on this PC', 'Nothing else found: no old copies, downloads or extra shortcuts');
  CacheRow := -1;
end;

// Ticked rows -> the lists the helper acts on; the fixed rows carry no finding.
procedure CollectChoices(List: TNewCheckListBox);
var
  Row, F: Integer;
  Kind: String;
begin
  SetArrayLength(ChosenPre, 0);
  SetArrayLength(ChosenPost, 0);
  SetArrayLength(ChosenFirewall, 0);
  for Row := 0 to List.Items.Count - 1 do begin
    if Row >= GetArrayLength(RowFinding) then
      break;
    F := RowFinding[Row];
    if (F < 0) or not List.Checked[Row] then
      continue;
    Kind := Field(Findings[F], 0);
    if Kind = 'firewall' then
      AppendLine(ChosenFirewall, Findings[F])
    else if Mode = ModeRepair then
      AppendLine(ChosenPost, Findings[F])
    else
      AppendLine(ChosenPre, Findings[F]);
  end;
  if DataRow >= 0 then
    KeepData := not List.Checked[DataRow];
  if CacheRow >= 0 then
    ClearCaches := List.Checked[CacheRow];
end;

procedure ChoiceClick(Sender: TObject);
begin
  if Sender = RepairButton then
    Mode := ModeRepair
  else if Sender = UninstallButton then
    Mode := ModeUninstall
  else
    Mode := ModeInstall;
  WizardForm.NextButton.OnClick(WizardForm.NextButton);
end;

function MakeChoice(const Caption, Hint: String; const Top: Integer): TNewButton;
var
  Grown: Integer;
begin
  Result := TNewButton.Create(ChoicePage);
  Result.Parent := ChoicePage.Surface;
  Result.Style := bsCommandLink;
  Result.Caption := Caption;
  Result.CommandLinkHint := Hint;
  Result.Left := 0;
  Result.Top := Top;
  Result.Width := ChoicePage.SurfaceWidth;
  Result.Height := ScaleY(64);
  Grown := Result.AdjustHeightIfCommandLink;
  Result.OnClick := @ChoiceClick;
end;

function MakeList(Page: TWizardPage): TNewCheckListBox;
begin
  Result := TNewCheckListBox.Create(Page);
  Result.Parent := Page.Surface;
  Result.Left := 0;
  Result.Top := 0;
  Result.Width := Page.SurfaceWidth;
  Result.Height := Page.SurfaceHeight;
  Result.Flat := True;
  Result.ShowLines := False;
end;

procedure InitializeWizard();
var
  InstallCaption, InstallHint: String;
  Y: Integer;
begin
  Done := TStringList.Create;
  Failures := TStringList.Create;
  InstalledVer := ReadInstalledVersion();

  if InstalledVer = '' then begin
    InstallCaption := 'Install Tin Hat';
    InstallHint := 'Installs Tin Hat {#AppVer} just for you - no administrator needed. After this it keeps itself up to date.';
  end else if InstalledVer = '{#AppVer}' then begin
    InstallCaption := 'Reinstall Tin Hat';
    InstallHint := 'Tin Hat {#AppVer} is already installed. This lays it down again; your sign-in and settings stay.';
  end else if ComparePackedVersion(PackVer(InstalledVer), PackVer('{#AppVer}')) < 0 then begin
    InstallCaption := 'Update Tin Hat';
    InstallHint := 'Updates Tin Hat ' + InstalledVer + ' to {#AppVer}. Your sign-in and settings stay.';
  end else begin
    InstallCaption := 'Install Tin Hat {#AppVer}';
    InstallHint := 'You have Tin Hat ' + InstalledVer + ', which is newer than this installer. It will update itself again afterwards.';
  end;

  ChoicePage := CreateCustomPage(wpWelcome, 'What would you like to do?',
    'Repair and Uninstall show you everything before they change anything.');
  InstallButton := MakeChoice(InstallCaption, InstallHint, 0);
  Y := InstallButton.Top + InstallButton.Height + ScaleY(8);
  RepairButton := MakeChoice('Repair Tin Hat',
    'For a Tin Hat that will not open or misbehaves: closes stuck copies, puts back every app file, and cleans up ' +
    'old copies from zip downloads and shortcuts that point at them. Your sign-in and settings stay.', Y);
  Y := RepairButton.Top + RepairButton.Height + ScaleY(8);
  UninstallButton := MakeChoice('Uninstall Tin Hat', 'Removes Tin Hat and everything it put on this PC.', Y);

  RepairPage := CreateCustomPage(ChoicePage.ID, 'Repair Tin Hat',
    'Ticked items are done; old copies and downloads go to the Recycle Bin. Your sign-in, keys and settings stay.');
  RepairList := MakeList(RepairPage);

  UninstallPage := CreateCustomPage(RepairPage.ID, 'Uninstall Tin Hat',
    'Ticked items are removed; old copies and downloads go to the Recycle Bin. Untick your sign-in to keep it.');
  UninstallList := MakeList(UninstallPage);
  UninstallNote := TNewStaticText.Create(UninstallPage);
  UninstallNote.Parent := UninstallPage.Surface;
  UninstallNote.WordWrap := True;
  UninstallNote.Left := 0;
  UninstallNote.Width := UninstallPage.SurfaceWidth;
  UninstallNote.Caption := 'Your messages stay on the server, but this PC''s encryption keys go with your sign-in: ' +
    'old encrypted messages stay readable only on your other signed-in devices or from your key backup.';
  Y := UninstallNote.AdjustHeight;
  UninstallNote.Top := UninstallPage.SurfaceHeight - UninstallNote.Height;
  UninstallList.Height := UninstallNote.Top - ScaleY(8);

  ScanPage := CreateOutputMarqueeProgressPage('Checking this PC', 'This takes a few seconds.');
end;

function InitializeSetup(): Boolean;
var
  M: String;
begin
  Result := True;
  ScannedMode := -1;
  DataRow := -1;
  CacheRow := -1;
  M := Lowercase(SwitchValue('/MODE'));
  if M = 'repair' then
    Mode := ModeRepair
  else if M = 'uninstall' then
    Mode := ModeUninstall
  else
    Mode := ModeInstall;
  // Interactive default: the sign-in goes (the user unticks it to keep it). Silent: kept
  // unless /REMOVEDATA - a script should never wipe someone's keys by omission.
  KeepData := WizardSilent and not HasSwitch('/REMOVEDATA');
  if HasSwitch('/KEEPDATA') then
    KeepData := True;
  ClearCaches := True;
  if WizardSilent then
    Log(Format('silent %s (keep sign-in: %d)', [ModeName(), Ord(KeepData)]));
end;

function ShouldSkipPage(PageID: Integer): Boolean;
begin
  Result := False;
  if (PageID = ChoicePage.ID) or (PageID = RepairPage.ID) or (PageID = UninstallPage.ID) then
    Result := WizardSilent;
  if PageID = RepairPage.ID then
    Result := Result or (Mode <> ModeRepair)
  else if PageID = UninstallPage.ID then
    Result := Result or (Mode <> ModeUninstall);
end;

function Plural(const N: Integer; const One, Many: String): String;
begin
  if N = 1 then
    Result := '1 ' + One
  else
    Result := IntToStr(N) + ' ' + Many;
end;

// The finish page says what happened, not just that it did.
function FinishText(): String;
var
  I: Integer;
begin
  Result := '';
  if InstallBroken then begin
    // What happened (and whether everything was put back) is in the list below.
    Result := 'Setup did not finish.';
  end else if Mode = ModeInstall then begin
    Result := 'It keeps itself up to date from now on.';
    for I := 0 to Done.Count - 1 do
      if Pos('runtime', Done[I]) > 0 then Result := Result + #13#10 + '- ' + Done[I];
    if RepairHint then
      Result := Result + #13#10#13#10 + 'Old copies of Tin Hat or shortcuts to them are still on this PC. ' +
        'Run this installer again and choose Repair to clean them up.';
    if ForeignSlot then
      Result := Result + #13#10#13#10 + 'The Start menu or the desktop already has a "Tin Hat" shortcut or link of your own ' +
        '(it opens something else), so Setup left it as it is and put none of its own there. Rename yours and run Repair ' +
        'to get Tin Hat''s, or open ' + RootDir() + '\current\tinhat.exe.';
  end else if Mode = ModeRepair then begin
    Result := 'Every app file was put back and the caches were reset.';
    for I := 0 to Done.Count - 1 do
      Result := Result + #13#10 + '- ' + Done[I];
    if ForeignSlot then
      Result := Result + #13#10 + '- Left your own "Tin Hat" shortcut or link (it opens something else) as it is: Tin Hat has none there';
    if DoneCopies > 0 then Result := Result + #13#10 + '- Moved ' + Plural(DoneCopies, 'old copy', 'old copies') + ' of Tin Hat to the Recycle Bin';
    if DoneRoot > 0 then Result := Result + #13#10 + '- Took the app files that were unzipped by hand out of the data folder';
    if DoneSandbox > 0 then Result := Result + #13#10 + '- Removed ' + Plural(DoneSandbox, 'hidden copy', 'hidden copies') + ' from another app''s sandbox';
    if DoneDownloads > 0 then Result := Result + #13#10 + '- Moved ' + Plural(DoneDownloads, 'old download', 'old downloads') + ' to the Recycle Bin';
    if DoneLinks > 0 then Result := Result + #13#10 + '- Pointed ' + Plural(DoneLinks, 'shortcut', 'shortcuts') + ' at the installed app';
    Result := Result + #13#10#13#10 + 'Your sign-in, keys and settings were kept.';
  end else begin
    Result := 'Tin Hat and what it put on this PC are gone.';
    if DoneCopies > 0 then Result := Result + #13#10 + '- Moved ' + Plural(DoneCopies, 'old copy', 'old copies') + ' of Tin Hat to the Recycle Bin';
    if DoneSandbox > 0 then Result := Result + #13#10 + '- Removed ' + Plural(DoneSandbox, 'hidden copy', 'hidden copies') + ' from another app''s sandbox';
    if DoneDownloads > 0 then Result := Result + #13#10 + '- Moved ' + Plural(DoneDownloads, 'download', 'downloads') + ' to the Recycle Bin';
    for I := 0 to Done.Count - 1 do
      if Pos('Firewall', Done[I]) > 0 then Result := Result + #13#10 + '- ' + Done[I];
    if KeepData then
      Result := Result + #13#10#13#10 + 'Your sign-in, keys and settings were kept in ' + RootDir() + ' for installing Tin Hat again later.'
    else if SignOutState = 1 then
      Result := Result + #13#10#13#10 + 'This PC was signed out of your account. Your messages stay on the server.'
    else if SignOutState = 3 then
      Result := Result + #13#10#13#10 + 'Setup could not reach the server to sign this PC out. Remove "Tin Hat (Windows)" from ' +
        'your account''s devices on another device. Your messages stay on the server.'
    else
      Result := Result + #13#10#13#10 + 'Your messages stay on the server.';
  end;
  if RestartNeeded then
    Result := Result + #13#10#13#10 + 'Restart Windows before you open Tin Hat.';
  if Failures.Count > 0 then begin
    Result := Result + #13#10#13#10 + 'Not finished:';
    for I := 0 to Failures.Count - 1 do
      if I < 6 then
        Result := Result + #13#10 + '- ' + Failures[I];
  end;
end;

procedure CurPageChanged(CurPageID: Integer);
var
  Grown: Integer;
begin
  // The choice page's own buttons replace Next there; every other page keeps Inno's say
  // (Next stays hidden while files are copied).
  if CurPageID = ChoicePage.ID then
    WizardForm.NextButton.Visible := False
  else if (CurPageID = RepairPage.ID) or (CurPageID = UninstallPage.ID) or (CurPageID = wpFinished) then
    WizardForm.NextButton.Visible := True;
  if CurPageID = ChoicePage.ID then
    WizardForm.ActiveControl := InstallButton
  else if CurPageID = RepairPage.ID then
    WizardForm.NextButton.Caption := 'Repair'
  else if CurPageID = UninstallPage.ID then
    WizardForm.NextButton.Caption := 'Uninstall'
  else if (CurPageID = wpPreparing) or (CurPageID = wpInstalling) then begin
    if Mode = ModeRepair then begin
      WizardForm.PageNameLabel.Caption := 'Repairing';
      WizardForm.PageDescriptionLabel.Caption := 'Please wait while Setup repairs Tin Hat.';
    end else if Mode = ModeUninstall then begin
      WizardForm.PageNameLabel.Caption := 'Uninstalling';
      WizardForm.PageDescriptionLabel.Caption := 'Please wait while Setup removes Tin Hat from this PC.';
    end;
  end else if CurPageID = wpFinished then begin
    if InstallBroken then
      WizardForm.FinishedHeadingLabel.Caption := 'Tin Hat could not be installed completely'
    else if Mode = ModeRepair then
      WizardForm.FinishedHeadingLabel.Caption := 'Tin Hat is repaired'
    else if Mode = ModeUninstall then
      WizardForm.FinishedHeadingLabel.Caption := 'Tin Hat is removed';
    WizardForm.FinishedLabel.Caption := FinishText();
    Grown := WizardForm.FinishedLabel.AdjustHeight;
    WizardForm.RunList.Top := WizardForm.RunList.Top + Grown;
    // Not until Windows has restarted for the runtime (or when an item did not finish).
    if RestartNeeded and (WizardForm.RunList.Items.Count > 0) then
      WizardForm.RunList.Checked[0] := False;
  end;
end;

function NextButtonClick(CurPageID: Integer): Boolean;
begin
  Result := True;
  if WizardSilent then
    exit; // silent runs scan and choose in PrepareToInstall
  if (CurPageID = ChoicePage.ID) and (Mode <> ModeInstall) then begin
    Scan(not WizardSilent);
    if Mode = ModeRepair then
      FillRepairList()
    else
      FillUninstallList();
  end else if CurPageID = RepairPage.ID then
    CollectChoices(RepairList)
  else if CurPageID = UninstallPage.ID then
    CollectChoices(UninstallList);
end;

// Before anything runs, nothing has changed - the stock "the program will not be installed"
// is wrong for Repair and Uninstall. (While files are being copied Inno's own wording and
// roll-back apply.)
procedure CancelButtonClick(CurPageID: Integer; var Cancel, Confirm: Boolean);
begin
  if CurPageID = wpInstalling then
    exit;
  Confirm := False;
  Cancel := MsgBox('Exit Setup? Nothing on this PC has been changed.', mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES;
end;

function PreviousDir(): String;
begin
  Result := RootDir() + '\setup-previous';
end;

// Inno rewrites its uninstall files and Setup opens the lock file: they must be plain files
// (no link, no second hard link). The helper checks both; if it cannot run at all, the
// link half is still checked here; any other answer than "fine" stops.
function MetadataSafe(): Boolean;
var
  Rc, I: Integer;
  Names: TArrayOfString;
  P: String;
  FindRec: TFindRec;
begin
  PrepareMaint();
  Rc := RunMaint('checkmeta', '');
  Result := Rc = 0;
  if Rc <> -1 then begin
    if not Result then
      Log(Format('checkmeta answered %d', [Rc]));
    exit;
  end;
  Log('checkmeta could not run - checking links only');
  Result := False;
  // A folder that cannot be listed cannot be checked. (FindFirst answers False both for "no
  // match" and for "cannot list"; a folder that can be listed always shows ".".)
  if DirExists(RootDir()) then begin
    if not FindFirst(RootDir() + '\*', FindRec) then begin
      Log('cannot list ' + RootDir());
      exit;
    end;
    FindClose(FindRec);
  end;
  Names := StringSplit('tinhat-maint.ps1|tinhat-bundle-files.txt|run\tinhat.lock', ['|'], stExcludeEmpty);
  // (Listing by pattern to check them - nothing is deleted by a pattern.)
  if FindFirst(RootDir() + '\unins???.*', FindRec) then begin
    try
      repeat
        AppendLine(Names, FindRec.Name);
      until not FindNext(FindRec);
    finally
      FindClose(FindRec);
    end;
  end;
  Result := True;
  for I := 0 to GetArrayLength(Names) - 1 do begin
    P := RootDir() + '\' + Names[I];
    if FileExists(P) and not IsRealPath(P) then begin
      Log(P + ' is reached through a link');
      Result := False;
    end;
  end;
end;

// The registry, read exactly: RegKeyExists answers False both for "not there" and for "cannot
// open", and Inno's own queries cannot tell REG_SZ from REG_EXPAND_SZ. (Setup is a 32-bit
// process, so a key handle fits a Longint; 0x109 = query values + list subkeys, 64-bit view.)
function RegOpenKeyExW(hKey: Longint; lpSubKey: String; ulOptions, samDesired: Longint; var phkResult: Longint): Longint;
  external 'RegOpenKeyExW@advapi32.dll stdcall';
function RegQueryValueExW(hKey: Longint; lpValueName: String; lpReserved: Longint; var lpType: Longint; lpData: Longint;
  var lpcbData: Longint): Longint;
  external 'RegQueryValueExW@advapi32.dll stdcall';
function RegCloseKey(hKey: Longint): Longint;
  external 'RegCloseKey@advapi32.dll stdcall';

// 0 = the key is there and can be read, 2 = it is not there, anything else (5: no access)
// = Setup cannot tell.
function KeyState(const Key: String): Longint;
var
  H: Longint;
begin
  H := 0;
  Result := RegOpenKeyExW(-2147483647 { HKEY_CURRENT_USER }, Key, 0, $109, H);
  if Result = 0 then
    RegCloseKey(H);
end;

// A value's registry type (1 REG_SZ, 2 REG_EXPAND_SZ, 4 REG_DWORD, ...); -1 = cannot be read.
function ValueKind(const Key, Name: String): Longint;
var
  H, T, Size: Longint;
begin
  Result := -1;
  H := 0;
  if RegOpenKeyExW(-2147483647 { HKEY_CURRENT_USER }, Key, 0, $109, H) <> 0 then
    exit;
  T := 0;
  Size := 0;
  if RegQueryValueExW(H, Name, 0, T, 0, Size) = 0 then
    Result := T;
  RegCloseKey(H);
end;

// A registry key's values as "S<name><TAB><text>" (REG_SZ), "E<name><TAB><text>"
// (REG_EXPAND_SZ) and "D<name><TAB><n>" (REG_DWORD). False = it cannot be copied completely:
// it cannot be read, or it holds a subkey or a value of another kind (neither Inno nor the
// app writes such things there) - Setup then does not rely on the copy.
function SnapshotKey(const Key: String; List: TStringList): Boolean;
var
  Names, Subs: TArrayOfString;
  I: Integer;
  V: String;
  D: Cardinal;
begin
  Result := False;
  List.Clear;
  if not RegGetValueNames(HKCU, Key, Names) or not RegGetSubkeyNames(HKCU, Key, Subs) then
    exit;
  if GetArrayLength(Subs) > 0 then begin
    Log(Key + ' holds a subkey');
    exit;
  end;
  for I := 0 to GetArrayLength(Names) - 1 do
    case ValueKind(Key, Names[I]) of
      1: if RegQueryStringValue(HKCU, Key, Names[I], V) then List.Add('S' + Names[I] + #9 + V) else exit;
      2: if RegQueryStringValue(HKCU, Key, Names[I], V) then List.Add('E' + Names[I] + #9 + V) else exit;
      4: if RegQueryDWordValue(HKCU, Key, Names[I], D) then List.Add('D' + Names[I] + #9 + IntToStr(D)) else exit;
    else
      begin
        Log(Key + ': the value "' + Names[I] + '" is of a kind Setup does not copy');
        exit;
      end;
    end;
  Result := True;
end;

// What a failed run must be able to put back exactly, as it is now: the Apps and features
// entries (ours and the in-app install's) and the toast identity key - existence, values and
// kinds; every unins???.exe/.dat/.msg in the data folder (Inno may reuse any matching one, not
// only the one the entry names); the helper files next to them. False = the copy is incomplete
// (something that cannot be read or copied completely), and Setup does not start changing things.
function BackUpKey(const Key: String; List: TStringList; var Existed: Boolean): Boolean;
var
  State: Integer;
begin
  Result := False;
  State := KeyState(Key);
  Existed := State = 0;
  if (State <> 0) and (State <> 2) then begin
    Log(Format('could not read %s (%d)', [Key, State]));
    exit;
  end;
  if Existed and not SnapshotKey(Key, List) then begin
    Log('could not copy ' + Key + ' completely');
    exit;
  end;
  Result := True;
end;

function SaveCopy(const Path: String; var Saved: String): Boolean;
begin
  Saved := ExpandConstant('{tmp}\entry-backup\') + ExtractFileName(Path);
  Result := FileCopy(Path, Saved, False);
  if not Result then
    Log('could not back up ' + Path);
end;

function BackUpEntry(): Boolean;
var
  FindRec: TFindRec;
  Ext, Saved: String;
  I: Integer;
begin
  Result := False;
  EntryBackup := TStringList.Create;
  LegacyBackup := TStringList.Create;
  AumidBackup := TStringList.Create;
  UninsBackup := TStringList.Create;
  HelperFiles := TStringList.Create;
  HelperSaved := TStringList.Create;
  if not BackUpKey(UninstallKey + EntryKey, EntryBackup, EntryExisted) or
     not BackUpKey(UninstallKey + InAppEntryKey, LegacyBackup, LegacyExisted) or
     not BackUpKey('Software\Classes\AppUserModelId\' + AUMID, AumidBackup, AumidExisted) then
    exit;
  if not ForceDirectories(ExpandConstant('{tmp}\entry-backup')) then begin
    Log('could not make ' + ExpandConstant('{tmp}\entry-backup'));
    exit;
  end;
  // (Listed by pattern to copy them - nothing is deleted by a pattern.)
  if FindFirst(RootDir() + '\unins???.*', FindRec) then begin
    try
      repeat
        Ext := Lowercase(ExtractFileExt(FindRec.Name));
        if (FindRec.Attributes and FILE_ATTRIBUTE_DIRECTORY = 0) and ((Ext = '.exe') or (Ext = '.dat') or (Ext = '.msg')) then begin
          if not SaveCopy(RootDir() + '\' + FindRec.Name, Saved) then
            exit;
          UninsBackup.Add(RootDir() + '\' + FindRec.Name);
        end;
      until not FindNext(FindRec);
    finally
      FindClose(FindRec);
    end;
  end;
  HelperFiles.Add(RootDir() + '\tinhat-maint.ps1');
  HelperFiles.Add(RootDir() + '\tinhat-bundle-files.txt');
  for I := 0 to HelperFiles.Count - 1 do begin
    Saved := '';
    if FileExists(HelperFiles[I]) and not SaveCopy(HelperFiles[I], Saved) then
      exit;
    HelperSaved.Add(Saved);
  end;
  EntrySaved := True;
  Result := True;
end;

// Put a key back as it was - including "it did not exist", and each value's kind. Touched:
// it had changed and was put back.
function RestoreKey(const Key: String; List: TStringList; const Existed: Boolean; var Touched: Boolean): Boolean;
var
  I, P: Integer;
  Line, Name, Value: String;
  Now: TStringList;
  Same: Boolean;
  D: Cardinal;
begin
  Result := True;
  // Unchanged (e.g. cancelled while copying files): leave it exactly as it is.
  if Existed then begin
    Now := TStringList.Create;
    try
      Same := (KeyState(Key) = 0) and SnapshotKey(Key, Now) and (Now.Text = List.Text);
    finally
      Now.Free;
    end;
  end else
    Same := KeyState(Key) = 2;
  if Same then
    exit;
  Touched := True;
  if (KeyState(Key) <> 2) and not RegDeleteKeyIncludingSubkeys(HKCU, Key) then begin
    Log('could not clear ' + Key);
    Result := False;
    exit;
  end;
  if not Existed then
    exit;
  // An entry that was there but empty is made again empty (writing a value creates the key).
  if List.Count = 0 then begin
    if not RegWriteStringValue(HKCU, Key, 'TinHatSetupTemp', '') or not RegDeleteValue(HKCU, Key, 'TinHatSetupTemp') then
      Result := False;
    exit;
  end;
  for I := 0 to List.Count - 1 do begin
    Line := List[I];
    P := Pos(#9, Line);
    Name := Copy(Line, 2, P - 2);
    Value := Copy(Line, P + 1, MaxInt);
    if Copy(Line, 1, 1) = 'S' then begin
      if not RegWriteStringValue(HKCU, Key, Name, Value) then Result := False;
    end else if Copy(Line, 1, 1) = 'E' then begin
      if not RegWriteExpandStringValue(HKCU, Key, Name, Value) then Result := False;
    end else begin
      D := StrToInt64Def(Value, 0);
      if not RegWriteDWordValue(HKCU, Key, Name, D) then Result := False;
    end;
  end;
end;

// Same bytes - a file Setup never got to rewrite is left exactly as it is.
function SameBytes(const A, B: String): Boolean;
begin
  Result := False;
  try
    Result := FileExists(A) and (GetSHA256OfFile(A) = GetSHA256OfFile(B));
  except
    Result := False;
  end;
end;

// Is Path one of the files backed up before this run (so it was there)?
function WasThere(const Path: String): Boolean;
var
  I: Integer;
begin
  Result := False;
  if UninsBackup <> nil then
    for I := 0 to UninsBackup.Count - 1 do
      if CompareText(UninsBackup[I], Path) = 0 then
        Result := True;
end;

// A file back as it was: its old bytes if it was there (never through a link), gone if it was
// not. True = done (or nothing to do).
function PutFileBack(const Path, Saved: String; var Touched: Boolean): Boolean;
begin
  Result := True;
  if Saved <> '' then begin
    if SameBytes(Path, Saved) then
      exit;
    Touched := True;
    if not FileExists(Path) and not IsRealPath(ExtractFileDir(Path)) then begin
      Log(ExtractFileDir(Path) + ' is reached through a link - ' + Path + ' not put back');
      Result := False;
    end else if FileExists(Path) and not IsRealPath(Path) then begin
      Log(Path + ' is reached through a link - not put back');
      Result := False;
    end else if not FileCopy(Saved, Path, False) then begin
      Log('could not put back ' + Path);
      Result := False;
    end;
  end else if FileExists(Path) then begin
    Touched := True;
    if not IsRealPath(Path) or not DeleteFile(Path) then begin
      Log('could not remove ' + Path + ' (added by this run)');
      Result := False;
    end;
  end;
end;

// Both Apps and features entries, the toast identity, every uninstaller file and the helper
// files back as they were before this run (what this run added goes). True = all of it.
function RestoreEntry(): Boolean;
var
  I: Integer;
  Touched: Boolean;
  Name, Ext, Saved: String;
begin
  Result := True;
  if not EntrySaved then
    exit;
  Touched := False;
  if not RestoreKey(UninstallKey + EntryKey, EntryBackup, EntryExisted, Touched) then Result := False;
  if not RestoreKey(UninstallKey + InAppEntryKey, LegacyBackup, LegacyExisted, Touched) then Result := False;
  if not RestoreKey('Software\Classes\AppUserModelId\' + AUMID, AumidBackup, AumidExisted, Touched) then Result := False;
  for I := 0 to UninsBackup.Count - 1 do
    if not PutFileBack(UninsBackup[I], ExpandConstant('{tmp}\entry-backup\') + ExtractFileName(UninsBackup[I]), Touched) then
      Result := False;
  // The uninstaller this run wrote, file by file: whatever of it was not there before goes.
  if BrokenUninstaller <> '' then begin
    Name := ChangeFileExt(BrokenUninstaller, '');
    if CompareText(ExtractFilePath(Name), AddBackslash(RootDir())) = 0 then
      for I := 0 to 2 do begin
        if I = 0 then Ext := '.exe' else if I = 1 then Ext := '.dat' else Ext := '.msg';
        if not WasThere(Name + Ext) and not PutFileBack(Name + Ext, '', Touched) then
          Result := False;
      end;
  end;
  for I := 0 to HelperFiles.Count - 1 do begin
    Saved := HelperSaved[I];
    if not PutFileBack(HelperFiles[I], Saved, Touched) then
      Result := False;
  end;
  if Result and not Touched then
    Log('the Apps and features entries, the uninstaller and the helper files were unchanged')
  else if Result then
    Log('put the previous Apps and features entries, uninstaller and helper files back')
  else
    Log('could not fully put the previous Apps and features entry back - run this installer again to repair it');
end;

// Every app file Inno reports as installed (the [Files] AfterInstall - called even for a file the
// user chose to skip after a copy error), so that AppComplete can check them all at the end.
procedure NoteInstalledFile();
var
  F: String;
begin
  if InstalledFiles = nil then
    InstalledFiles := TStringList.Create;
  F := ExpandConstant(CurrentFileName);
#ifdef GateBuild
  if (GetEnv('TH3GATE_SKIP') <> '') and (CompareText(ExtractFileName(F), GetEnv('TH3GATE_SKIP')) = 0) then begin
    Log('gate build: taking away ' + F + ' as if it had been skipped');
    DeleteFile(F);
  end;
  // Held open (no sharing) until Setup exits, so that putting things back fails.
  if (GetEnv('TH3GATE_HOLD') <> '') and (CompareText(ExtractFileName(F), GetEnv('TH3GATE_HOLD')) = 0) then begin
    GateHold := CreateFileW(F, -2147483647 - 1 { GENERIC_READ }, 0, 0, 3 { OPEN_EXISTING }, 0, 0);
    Log('gate build: holding ' + F + ' open');
  end;
#endif
  InstalledFiles.Add(F);
end;

// Every file of this bundle is where Inno put it: none skipped after a copy error, none taken
// away since (antivirus). Nothing old is let go on the strength of an incomplete app.
function AppComplete(): Boolean;
var
  I, Missing: Integer;
begin
  Missing := 0;
  if InstalledFiles <> nil then
    for I := 0 to InstalledFiles.Count - 1 do
      if not FileExists(InstalledFiles[I]) then begin
        Missing := Missing + 1;
        if Missing <= 5 then
          Log('missing after the install step: ' + InstalledFiles[I]);
      end;
  // The helper files must be this Setup's (a skipped replacement leaves an older one), and the
  // uninstaller the entry names must be there.
  if not SameBytes(RootDir() + '\tinhat-maint.ps1', ExpandConstant('{tmp}\tinhat-maint.ps1')) then begin
    Log('the helper in ' + RootDir() + ' is not this Setup''s');
    Missing := Missing + 1;
  end;
  if not SameBytes(RootDir() + '\tinhat-bundle-files.txt', ExpandConstant('{tmp}\tinhat-bundle-files.txt')) then begin
    Log('the bundle file list in ' + RootDir() + ' is not this Setup''s');
    Missing := Missing + 1;
  end;
  try
    if not FileExists(ExpandConstant('{uninstallexe}')) or not FileExists(ChangeFileExt(ExpandConstant('{uninstallexe}'), '.dat')) then begin
      Log('the uninstaller or its log is missing: ' + ExpandConstant('{uninstallexe}'));
      Missing := Missing + 1;
    end;
  except
    Log('no uninstaller was written');
    Missing := Missing + 1;
  end;
  Result := (InstalledFiles <> nil) and (InstalledFiles.Count > 0) and (Missing = 0);
  if (InstalledFiles = nil) or (InstalledFiles.Count = 0) then
    Log('no app files were reported as installed')
  else if Missing > 0 then
    Log(Format('%d of %d app files missing after the install step', [Missing, InstalledFiles.Count]));
end;

// The finish page's "Open Tin Hat": only for an app that was installed completely.
function AppReady(): Boolean;
begin
  Result := (not IsUninstallMode) and InstallFinished and not RuntimeMissing;
end;

// Silent callers (and the gate) can tell: 9 = an app file was missing at the end and the run was
// put back; 10 = installed, but the Visual C++ runtime is missing, so nothing old was let go.
function GetCustomSetupExitCode(): Integer;
begin
  Result := 0;
  if InstallBroken then
    Result := 9
  else if RuntimeMissing then
    Result := 10;
end;

// The installer's own shortcut slots ("Tin Hat" in the Start menu and on the desktop) as they
// were before [Icons] replaced them - put back exactly if this run does not finish.
procedure BackUpSlots();
var
  I: Integer;
  Saved: String;
begin
  SlotPaths := TStringList.Create;
  SlotSaved := TStringList.Create;
  SlotChanged := TStringList.Create;
  SlotPaths.Add(ExpandConstant('{userprograms}\Tin Hat.lnk'));
  SlotPaths.Add(ExpandConstant('{userdesktop}\Tin Hat.lnk'));
  SlotChanged.Add('0');
  SlotChanged.Add('0');
  ForceDirectories(ExpandConstant('{tmp}\slot-backup'));
  for I := 0 to SlotPaths.Count - 1 do begin
    Saved := '';
    if FileExists(SlotPaths[I]) then begin
      Saved := ExpandConstant('{tmp}\slot-backup\') + IntToStr(I) + '.lnk';
      if not FileCopy(SlotPaths[I], Saved, False) then begin
        Log('could not keep a copy of ' + SlotPaths[I]);
        Saved := '?';
      end;
    end;
    SlotSaved.Add(Saved);
  end;
end;

// The slots [Icons] rewrote, back as they were: a replaced shortcut byte for byte, one this run
// made removed (OnlyReplaced: leave those). Never through a folder or file that is a link.
function RestoreSlots(const OnlyReplaced: Boolean): Boolean;
var
  I: Integer;
  Lnk, Saved: String;
  Touched: Boolean;
begin
  Result := True;
  if (SlotPaths = nil) or (SlotChanged = nil) then
    exit;
  for I := 0 to SlotPaths.Count - 1 do begin
    if SlotChanged[I] <> '1' then
      continue;
    Lnk := SlotPaths[I];
    Saved := SlotSaved[I];
    if (Saved = '') and OnlyReplaced then
      continue;
    if not IsRealPath(ExtractFileDir(Lnk)) then begin
      Log(ExtractFileDir(Lnk) + ' is reached through a link - ' + Lnk + ' not put back');
      Result := False;
    end else if Saved <> '' then begin
      Touched := False;
      if PutFileBack(Lnk, Saved, Touched) then begin
        if Touched then Log('put back ' + Lnk);
      end else
        Result := False;
    end else if FileExists(Lnk) and IsRealPath(Lnk) and ShortcutOpensTinHat(Lnk) then begin
      if DeleteFile(Lnk) then
        Log('removed ' + Lnk + ' (made by this run)')
      else begin
        Log('could not remove ' + Lnk);
        Result := False;
      end;
    end;
    SlotChanged[I] := '0';
  end;
end;

// A "Tin Hat" shortcut in the Start menu or on the desktop that [Icons] did not replace (it
// could not be copied first, or writing it failed) and that still opens an old copy keeps that
// copy: removing it would leave the shortcut people use broken. The finish page says so.
procedure KeepCopiesStillOpened();
var
  I, J: Integer;
  Kind, Dir, Target: String;
  Keep: Boolean;
  Kept: TArrayOfString;
begin
  if SlotPaths = nil then
    exit;
  SetArrayLength(Kept, 0);
  for I := 0 to GetArrayLength(ChosenPost) - 1 do begin
    Kind := Field(ChosenPost[I], 0);
    Keep := False;
    if (Kind = 'copy') or (Kind = 'copymixed') or (Kind = 'rootmix') then begin
      Dir := AddBackslash(Field(ChosenPost[I], 2));
      for J := 0 to SlotPaths.Count - 1 do
        if FileExists(SlotPaths[J]) then begin
          Target := ShortcutTarget(SlotPaths[J]);
          // The copy's own exe (right in that folder - the installed app in current\ under the
          // data folder does not count).
          if (Target <> '') and (CompareText(ExtractFilePath(Target), Dir) = 0) then
            Keep := True;
        end;
    end;
    if Keep then begin
      Log('keeping ' + Field(ChosenPost[I], 2) + ': a "Tin Hat" shortcut Setup could not replace still opens it');
      Failures.Add('Kept the old copy in ' + Field(ChosenPost[I], 2) + ': a "Tin Hat" shortcut Setup could not replace still opens it');
    end else
      AppendLine(Kept, ChosenPost[I]);
  end;
  ChosenPost := Kept;
end;

// A folder rename that a moment's hold on a file inside may block: antivirus or the search
// indexer reading an app file that was just written (seen on a busy PC right after an install).
// A few tries over about ten seconds before giving up; a lasting hold still fails, as before.
function RenameWithRetries(const FromDir, ToDir: String): Boolean;
var
  I: Integer;
begin
  Result := False;
  for I := 1 to 20 do begin
    Result := RenameFile(FromDir, ToDir);
    if Result then
      exit;
    if I = 1 then
      Log('could not rename ' + FromDir + ' yet (a file in it is in use?) - trying again for a few seconds');
    Sleep(500);
  end;
end;

// A folder name nothing else uses, for keeping a current\ of unknown completeness aside.
function FreeInterruptedName(): String;
var
  I: Integer;
begin
  Result := RootDir() + '\setup-interrupted';
  for I := 1 to 3 do
    if DirExists(Result) or FileExists(Result) then
      Result := RootDir() + '\setup-interrupted-' + IntToStr(I);
  if DirExists(Result) or FileExists(Result) then
    Result := '';
end;

// The installed bundle is set aside, not deleted, before the new files go in, so a failed
// or cancelled Setup can put it back (DeinitializeSetup). Only a rename this run made is
// ever restored, and nothing that might be the last working copy is ever deleted before
// this run has succeeded.
function SetCurrentAside(): String;
var
  Cur: String;
begin
  Result := '';
  Cur := RootDir() + '\current';
  if DirExists(PreviousDir()) then begin
    // An earlier Setup set the app aside and never finished (power loss, killed). That copy
    // was complete when it was set aside; current\ may be half-copied. Keep both until this
    // run succeeds: the earlier copy is the fallback, current\ waits in setup-interrupted.
    Log('found the app set aside by an earlier Setup that did not finish - keeping it as the fallback');
    if DirExists(Cur) then begin
      InterruptedDir := FreeInterruptedName();
      if (InterruptedDir = '') or not RenameWithRetries(Cur, InterruptedDir) then begin
        InterruptedDir := '';
        Result := 'An earlier Tin Hat Setup did not finish and its files are still in use.' + #13#10#13#10 +
          'Restart Windows, then run this installer again.';
        exit;
      end;
      Log('kept the current app aside in ' + InterruptedDir + ' until this Setup finishes');
    end;
    SetAsideDone := True;
    exit;
  end;
  if not DirExists(Cur) then begin
    FreshInstall := True;
    exit;
  end;
  if RenameWithRetries(Cur, PreviousDir()) then begin
    SetAsideDone := True;
    Log('set the installed app aside in ' + PreviousDir());
  end else begin
    Log('could not set current\ aside');
    Result := 'Some of Tin Hat''s files are in use by another program, so Setup cannot replace them.' + #13#10#13#10 +
      'Restart Windows, then run this installer again.';
  end;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Why: String;
begin
  Result := '';
  // The folder page is hidden, but /DIR= or /LOADINF can still move {app}; everything
  // below (setting current\ aside included) must only ever touch Tin Hat's own folder.
  if CompareText(RemoveBackslashUnlessRoot(ExpandConstant('{app}')), RootDir() + '\current') <> 0 then begin
    Log('refusing: {app} is ' + ExpandConstant('{app}'));
    Result := 'This installer always puts Tin Hat in ' + RootDir() + '\current, but it was started with a different ' +
      'folder (/DIR). Run it again without that option.';
    exit;
  end;
  if WizardSilent and (Mode <> ModeInstall) then begin
    Scan(False);
    ChooseDefaults();
  end;
  // Inno rewrites its uninstall files and Setup opens the lock: they must be plain files
  // here (no link, no second hard link to something else). Both checks come before Tin Hat
  // is closed, so a refusal really changes nothing.
  if not MetadataSafe() then begin
    Result := 'Some of Tin Hat Setup''s own files in ' + RootDir() + ' could not be confirmed as plain files (a link ' +
      'to somewhere else?), so Setup will not write to them. See "Setup Log" in %TEMP%, then run this installer again.';
    exit;
  end;
  if (Mode <> ModeUninstall) and not BackUpEntry() then begin
    Result := 'Setup could not make a safety copy of Tin Hat''s Apps and features entry, so it will not change ' +
      'anything. See "Setup Log" in %TEMP%, then run this installer again.';
    exit;
  end;
  // Interactive: the Visual C++ runtime before anything changes - without it Tin Hat cannot
  // start, so nothing that works today is let go for an app that could not.
  if (Mode <> ModeUninstall) and not PutRuntimeInPlaceFirst(Why) then begin
    Result := Why;
    exit;
  end;
  if CloseTinHat() then
    Done.Add('Closed Tin Hat, which was still running');
  // Hold the lock from here to the end: a Tin Hat started meanwhile quits at once instead
  // of loading half-replaced files, and nothing below can race one.
  if not TakeLockWithRetries() then begin
    Log('tinhat.lock is still held - not continuing');
    Result := 'Tin Hat is still running in the background and could not be closed.' + #13#10#13#10 +
      'Restart Windows, then run this installer again.';
    exit;
  end;
  if Mode <> ModeUninstall then
    Result := SetCurrentAside();
end;

// '...' for PowerShell: a single-quoted string, quotes doubled.
// '...' for PowerShell with every quote character it accepts doubled - only ever used for
// values Setup itself produced (our own folder names), never for elevated command text.
function PSQuote(const S: String): String;
var
  T: String;
begin
  T := S;
  StringChangeEx(T, '''', '''''', True);
  StringChangeEx(T, #$2018, #$2018#$2018, True);
  StringChangeEx(T, #$2019, #$2019#$2019, True);
  StringChangeEx(T, #$201A, #$201A#$201A, True);
  StringChangeEx(T, #$201B, #$201B#$201B, True);
  Result := '''' + T + '''';
end;

// A string as UTF-16 hex: pure data, nothing in it can end a quote or start a command.
function HexOf(const S: String): String;
var
  I: Integer;
begin
  Result := '';
  for I := 1 to Length(S) do
    Result := Result + Format('%.4x', [Ord(S[I])]);
end;

// Remove the rules whose program is one of the chosen copies (paths from the scan; a
// Windows path cannot contain the double quote that would end the argument).
// Remove the rules whose program is one of the chosen copies. It runs elevated, so: the
// paths arrive as hex (decoded inside), only Windows' own NetSecurity module is loaded (by
// full path, the module search path pinned to Windows' folder) and every command is called
// by its module-qualified name - nothing the unelevated user controls can stand in.
function FirewallCommand(): String;
var
  I: Integer;
  List, Win: String;
begin
  List := '';
  for I := 0 to GetArrayLength(ChosenFirewall) - 1 do begin
    if List <> '' then
      List := List + ',';
    List := List + '''' + HexOf(Field(ChosenFirewall[I], 2)) + '''';
  end;
  Win := HexOf(ExpandConstant('{win}'));
  Result :=
    '$ErrorActionPreference = ''Stop''; ' +
    '$h = { param($x) -join ((0..($x.Length / 4 - 1)) | Microsoft.PowerShell.Core\ForEach-Object { [char][Convert]::ToInt32($x.Substring($_ * 4, 4), 16) }) }; ' +
    '$w = & $h ''' + Win + '''; ' +
    '$env:PSModulePath = $w + ''\System32\WindowsPowerShell\v1.0\Modules''; ' +
    'Microsoft.PowerShell.Core\Import-Module ($w + ''\System32\WindowsPowerShell\v1.0\Modules\NetSecurity\NetSecurity.psd1''); ' +
    '$a = @(@(' + List + ') | Microsoft.PowerShell.Core\ForEach-Object { & $h $_ }); ' +
    'NetSecurity\Get-NetFirewallApplicationFilter | Microsoft.PowerShell.Core\Where-Object { $a -contains ' +
    '[Environment]::ExpandEnvironmentVariables($_.Program) } | NetSecurity\Get-NetFirewallRule | NetSecurity\Remove-NetFirewallRule';
end;

procedure DropSandboxes();
var
  Kept: TArrayOfString;
  I: Integer;
begin
  SetArrayLength(Kept, 0);
  for I := 0 to GetArrayLength(ChosenPre) - 1 do
    if Field(ChosenPre[I], 0) <> 'vcopy' then
      AppendLine(Kept, ChosenPre[I]);
  ChosenPre := Kept;
end;

procedure UninstallSteps();
var
  F: String;
  Code: Integer;
begin
  try
    if not KeepData then begin
      SetStatus('Signing this PC out of Tin Hat...');
      F := ExpandConstant('{tmp}\tinhat-chosen-signout.txt');
      if SaveStringsToUTF8File(F, ChosenPre, False) then
        RunMaint('signout', '-In "' + F + '"')
      else begin
        // Without the list the hidden copies cannot be signed out: they keep their data.
        RunMaint('signout', '');
        DropSandboxes();
        Failures.Add('Hidden copies were left alone: Setup could not hand them to its clean-up helper');
      end;
    end;
    if GetArrayLength(ChosenPre) > 0 then begin
      SetStatus('Removing old copies, downloads and shortcuts...');
      ApplyList(ChosenPre, 'tinhat-chosen.txt');
    end;
  except
    Log('clean-up helper step failed: ' + GetExceptionMessage);
  end;
  if GetArrayLength(ChosenFirewall) > 0 then begin
    SetStatus('Removing the Windows Firewall rules (Windows asks for permission)...');
    F := ExpandConstant('{tmp}\tinhat-firewall.txt');
    if not SaveStringsToUTF8File(F, ChosenFirewall, False) then begin
      Log('could not write ' + F);
      Failures.Add('The Windows Firewall rules for Tin Hat were not removed (Setup could not prepare the list)');
      SetArrayLength(ChosenFirewall, 0);
    end;
  end;
  if GetArrayLength(ChosenFirewall) > 0 then begin
    // The elevated PowerShell runs code from its own command line - nothing it reads or
    // writes lives in a folder this (unelevated) user could swap while Windows asks.
    if ShellExec('runas', PowerShellExe(), '-NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "' +
         FirewallCommand() + '"', '', SW_HIDE, ewWaitUntilTerminated, Code) then begin
      Code := RunMaint('fwcheck', '-In "' + F + '"');
      if Code = 0 then
        Done.Add('Removed the Windows Firewall rules for Tin Hat')
      else if Code = 255 then
        Failures.Add('Could not check the Windows Firewall rules for Tin Hat afterwards')
      else
        Failures.Add('Some Windows Firewall rules for Tin Hat could not be removed');
    end else begin
      Log(Format('firewall clean-up not run (%s)', [SysErrorMessage(Code)]));
      Failures.Add('The Windows Firewall rules for Tin Hat stay: administrator permission was not given (run Setup again to remove them)');
    end;
  end;
  RemoveFootprint(KeepData);
  SetStatus('Clearing what Windows recorded about Tin Hat...');
  try
    RunMaint('traces', '');
  except
    Log('clean-up helper step failed: ' + GetExceptionMessage);
  end;
end;


// The installed bundle is set aside, not deleted, before the new files go in: if copying
// fails or is cancelled, DeinitializeSetup puts it back - a failed Repair never leaves a
// PC with no working Tin Hat. (Nothing may be left of an older bundle next to the new one.)

// Put back what was there before this run: the previous app (or no app), the Apps and features
// entries, the toast identity, the uninstaller and helper files, the "Tin Hat" shortcut slots.
// Runs once - right away when the app turns out incomplete, else when Setup ends without having
// finished. True = everything is back.
function RollBack(): Boolean;
var
  Cur: String;
begin
  Result := True;
  if RolledBack or (Mode = ModeUninstall) or InstallFinished then
    exit;
  RolledBack := True;
  Cur := RootDir() + '\current';
  if SetAsideDone and DirExists(PreviousDir()) then begin
    DeleteTree(Cur);
    if (not DirExists(Cur)) and RenameWithRetries(PreviousDir(), Cur) then
      Log('Setup did not finish - the previous Tin Hat was put back')
    else begin
      Log('Setup did not finish - could not put the previous Tin Hat back from ' + PreviousDir());
      Result := False;
    end;
    if InterruptedDir <> '' then
      Log('a copy of unknown completeness stays in ' + InterruptedDir);
  end else if FreshInstall and DirExists(Cur) then begin
    // Nothing was there before this run: its files go (Inno's own roll-back does not remove
    // them - they are uninsneveruninstall).
    DeleteTree(Cur);
    if DirExists(Cur) then begin
      Log('Setup did not finish - could not remove all the files it had copied');
      Result := False;
    end else
      Log('Setup did not finish - removed the files it had copied');
  end;
  if RegistryTouched then begin
    if not RestoreEntry() then
      Result := False;
    if not RestoreSlots(False) then
      Result := False;
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  Ready: Boolean;
  Msg: String;
begin
  if CurStep = ssInstall then begin
    RegistryTouched := True;
    PrepareMaint();
    if Mode = ModeUninstall then
      UninstallSteps()
    else begin
      // Updater leftovers, the GStreamer plugin cache, Qt's compiled-QML/pipeline caches and
      // the safe-mode start marker (a stale "starting" line would put the first start of the
      // same version in safe mode) - all rebuilt by the app itself.
      DeleteTree(RootDir() + '\backup');
      DeleteTree(RootDir() + '\staging');
      DeleteOneFile(RootDir() + '\gstregistry.bin');
      DeleteTree(ExpandConstant('{localappdata}\TinHat\tinhat\cache'));
      DeleteOneFile(RootDir() + '\config\tinhat\boot-state');
      BackUpSlots();
    end;
  end else if CurStep = ssPostInstall then begin
    if Mode = ModeUninstall then begin
      // Setup created the (empty) app folder; nothing of Tin Hat stays behind.
      DeleteTree(ExpandConstant('{app}'));
      RemoveIfEmpty(RootDir());
      exit;
    end;
    // Every app file must be in place before anything old is let go. One skipped after a copy
    // error or taken away since (antivirus) makes this run "not finished": DeinitializeSetup
    // puts back what was there, the lock is kept until then, and nothing outside Tin Hat's
    // folder has been changed.
    if not AppComplete() then begin
      InstallBroken := True;
      try
        BrokenUninstaller := ExpandConstant('{uninstallexe}');
      except
        BrokenUninstaller := '';
      end;
      // Put back right away (the lock still held) - the finish page then tells what happened.
      if RollBack() then
        Msg := 'Some of Tin Hat''s files could not be installed (a file was skipped after an error, or antivirus ' +
          'took it away), so Setup put back what was there before. See "Setup Log" in %TEMP%, then run this installer again.'
      else if DirExists(PreviousDir()) then
        Msg := 'Some of Tin Hat''s files could not be installed, and Setup could not put everything back - the ' +
          'previous Tin Hat is kept in ' + PreviousDir() + '. Restart Windows, then run this installer again.'
      else
        Msg := 'Some of Tin Hat''s files could not be installed, and Setup could not put everything back (see ' +
          '"Setup Log" in %TEMP%). Restart Windows, then run this installer again.';
      Log('not finished: ' + Msg);
      Failures.Add(Msg);
      ReleaseLock();
    end else begin
      InstallFinished := True;
      try
        // The Visual C++ runtime first: a copy that runs today is not let go for an app that
        // cannot start yet.
        EnsureVCRuntime();
        Ready := VCRuntimeOK() or RestartNeeded;
        if not Ready then begin
          // (Only a silent run gets here.) The app stays, but everything a working copy is
          // reached by stays too: shortcuts it replaced go back, and the set-aside app is kept.
          RuntimeMissing := True;
          if not RestoreSlots(True) then
            Failures.Add('A "Tin Hat" shortcut Setup had replaced could not be put back (see "Setup Log" in %TEMP%).');
        end;
        // Only now does anything outside Tin Hat's folder change: for Repair, what was ticked -
        // old copies and downloads go to the Recycle Bin, shortcuts and pins are pointed at the
        // installed app. A Repair that fails, is cancelled or finds the app incomplete leaves
        // them all as they were, so the copy someone ran from Downloads still works.
        if Mode = ModeRepair then begin
          if Ready then
            KeepCopiesStillOpened();
          if GetArrayLength(ChosenPost) > 0 then begin
            if Ready then begin
              SetStatus('Removing old copies and downloads, pointing shortcuts at the installed app...');
              ApplyList(ChosenPost, 'tinhat-chosen-post.txt');
            end else
              Failures.Add('Old copies, downloads and shortcuts were left as they are until the Visual C++ runtime is ' +
                'installed - run Repair again then.');
          end;
          RepairFootprint();
        end;
        if Mode = ModeInstall then begin
          // Nothing is changed here - only a pointer to Repair if old copies are about.
          SetStatus('Checking for old copies of Tin Hat...');
          Scan(False);
          ChooseDefaults();
          RepairHint := GetArrayLength(ChosenPre) + GetArrayLength(ChosenPost) > 0;
        end;
        // Only now is the set-aside copy (and a half-copied one an earlier run left) not needed.
        if Ready then begin
          DeleteTree(PreviousDir());
          if InterruptedDir <> '' then
            DeleteTree(InterruptedDir);
        end else
          Log('keeping ' + PreviousDir() + ' until the Visual C++ runtime is there');
      except
        Log('after the install step: ' + GetExceptionMessage);
        Failures.Add('Setup hit an error after installing Tin Hat (see "Setup Log" in %TEMP%) - run this installer again.');
      end;
      // The app takes its own lock again when it starts ("Open Tin Hat" on the last page).
      ReleaseLock();
    end;
  end;
end;

procedure DeinitializeSetup();
var
  Msg: String;
begin
  if not RollBack() then begin
    Msg := 'Setup did not finish, and it could not put everything back as it was.';
    if DirExists(PreviousDir()) then
      Msg := Msg + ' The previous Tin Hat is kept in ' + PreviousDir() + '.';
    Msg := Msg + #13#10#13#10 + 'Restart Windows, then run this installer again. (Details: "Setup Log" in %TEMP%.)';
    Log('not finished: ' + Msg);
    if not WizardSilent then
      MsgBox(Msg, mbError, MB_OK);
  end;
  ReleaseLock();
  // Setup's Uninstall: the lock's folder goes once the lock is let go.
  if (Mode = ModeUninstall) and RemovalDone then begin
    DeleteTree(RootDir() + '\run');
    RemoveIfEmpty(RootDir());
  end;
end;

// ---------------------------------------------------------------------------------
// The Apps & features uninstaller (unins000.exe) runs the same removal
// ---------------------------------------------------------------------------------

function InitializeUninstall(): Boolean;
begin
  Result := True;
  Mode := ModeUninstall;
  ScannedMode := -1;
  Done := TStringList.Create;
  Failures := TStringList.Create;
  // Silent: the sign-in stays unless /REMOVEDATA. Interactive: asked after the confirm.
  KeepData := not HasSwitch('/REMOVEDATA');
end;

// Inno's own "Remove Tin Hat from this PC?" is the one confirmation; this only asks what
// happens to the sign-in.
procedure AskAboutSignIn();
var
  Remove, Keep: String;
begin
  // (No line of [Code] may start with a bracket: ISCC reads it as a section tag.)
  Remove := 'Remove it too'#10'Signs this PC out and deletes your sign-in, keys and settings. Your messages stay on the server.';
  Keep := 'Keep it'#10'For installing Tin Hat again later.';
  KeepData := TaskDialogMsgBox('Keep your sign-in on this PC?',
    'Tin Hat is being removed. Its encryption keys go with your sign-in: old encrypted messages then stay readable ' +
    'only on your other signed-in devices or from your key backup.',
    mbConfirmation, MB_YESNO, [Remove, Keep], 0) = IDNO;
end;

// The file name of the uninstaller that is running (unins000.exe, or unins001.exe ...).
function UninsName(): String;
begin
  Result := 'unins000.exe';
  try
    Result := ExtractFileName(ExpandConstant('{uninstallexe}'));
  except
    Log('could not tell the uninstaller''s own name - assuming unins000.exe');
  end;
end;

procedure DeinitializeUninstall();
begin
  ReleaseLock();
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  I: Integer;
  Msg: String;
begin
  if CurUninstallStep = usUninstall then begin
    // The finding and cleaning of extras must never stand between the user and the
    // removal itself: any error there is logged and the removal carries on.
    try
      CloseTinHat();
    except
      Log('closing Tin Hat failed: ' + GetExceptionMessage);
    end;
    if not MetadataSafe() then begin
      Log('Setup''s own files could not be confirmed as plain files - uninstall stopped before changing anything');
      if not UninstallSilent then
        MsgBox('Some of Tin Hat''s own files in ' + RootDir() + ' are links to somewhere else, so the uninstaller ' +
          'will not touch them. Remove them (or ask for help), then uninstall again.', mbError, MB_OK);
      Abort;
    end;
    // A copy this user cannot end (started as administrator) still holds the lock and its
    // files: stop before anything changes rather than half-remove a running app. Taking
    // the lock also keeps Tin Hat from starting again while it is being removed.
    if not TakeLockWithRetries() then begin
      Log('tinhat.lock is still held - uninstall stopped before changing anything');
      if not UninstallSilent then
        MsgBox('Tin Hat is still running in the background and could not be closed.' + #13#10#13#10 +
          'Restart Windows, then uninstall Tin Hat again.', mbError, MB_OK);
      Abort;
    end;
    if not UninstallSilent then
      AskAboutSignIn();
    Log(Format('uninstall (keep sign-in: %d)', [Ord(KeepData)]));
    try
      SetStatus('Looking for old copies of Tin Hat...');
      Scan(False);
      ChooseDefaults();
    except
      Log('clean-up helper step failed: ' + GetExceptionMessage);
      SetArrayLength(ChosenPre, 0);
    end;
    UninstallSteps();
  end else if CurUninstallStep = usPostUninstall then begin
    // Inno's own removal is done too: let go of the lock, then its folder can go.
    ReleaseLock();
    DeleteTree(RootDir() + '\run');
    // Inno deletes unins000.exe only once this code is done, and leaves the folder behind:
    // a hidden helper waits for that, then removes the folder - if nothing else is in it.
    if not KeepData then
      Exec(PowerShellExe(), '-NoProfile -NonInteractive -WindowStyle Hidden -Command "$d = ' + PSQuote(RootDir()) +
        '; for ($i = 0; $i -lt 120 -and (Test-Path -LiteralPath (Join-Path $d ' + PSQuote(UninsName()) + ')); $i++) ' +
        '{ Start-Sleep -Milliseconds 500 }; if (@(Get-ChildItem -LiteralPath $d -Force -ErrorAction SilentlyContinue).Count -eq 0) ' +
        '{ Remove-Item -LiteralPath $d -Force -ErrorAction SilentlyContinue }"', '', SW_HIDE, ewNoWait, I);
    if (SignOutState = 3) and not KeepData then
      Failures.Add('Could not reach the server to sign this PC out - remove "Tin Hat (Windows)" from your ' +
        'account''s devices on another device');
    if (Failures.Count > 0) and not UninstallSilent then begin
      Msg := 'Tin Hat is removed, but these did not finish:';
      for I := 0 to Failures.Count - 1 do
        if I < 6 then
          Msg := Msg + #13#10 + '- ' + Failures[I];
      MsgBox(Msg, mbInformation, MB_OK);
    end;
  end;
end;
