# run-tinhat-tx-test.ps1 ARTIFACT_DIR [-v] — the Windows twin of run-tinhat-tx-test.sh: compile tinhat_tx_test.cc with
# MSVC against a BUILT win-x64 libwebrtc archive (the folder win-build-patched.cmd wrote: lib\webrtc.lib, include\,
# webrtc.ninja) and run it on the shared vectors testvectors\tx-len\vectors.json. It drives that archive's own
# FrameCryptorTransformer, so it tests the sender that ships. The MSVC environment comes from the repo's
# windows\dev-env.ps1 when cl.exe isn't on PATH yet; /MT like the app (.cargo/config.toml crt-static) and the archive.
# Exit 0 PASS, 1 sender FAIL, 2 harness/control problem.
param([Parameter(Mandatory = $true)][string]$Artifact, [switch]$v)
$ErrorActionPreference = 'Stop'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$repo = (Resolve-Path (Join-Path $here '..\..\..\..')).Path
$A = (Resolve-Path $Artifact).Path
if (!(Test-Path "$A\lib\webrtc.lib") -or !(Test-Path "$A\include") -or !(Test-Path "$A\webrtc.ninja")) {
  Write-Output "not a libwebrtc artifact dir: $A"; exit 2
}
if (-not (Get-Command cl.exe -ErrorAction SilentlyContinue)) { . (Join-Path $repo 'windows\dev-env.ps1') | Out-Null }
# the archive's own defines (as webrtc-sys/build.rs passes them), minus quoted values (build-id strings)
$line = (Select-String -Path "$A\webrtc.ninja" -Pattern '^defines = ' | Select-Object -First 1).Line.Substring(10)
$defs = @($line -split ' ' | Where-Object { $_ -like '-D*' -and $_ -notmatch '"' -and $_ -notmatch '\\' })
$t = Join-Path $env:TEMP ('tinhat-tx-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory $t | Out-Null
try {
  $libs = @('webrtc.lib', 'msdmo.lib', 'wmcodecdspuuid.lib', 'dmoguids.lib', 'crypt32.lib', 'iphlpapi.lib', 'ole32.lib',
    'secur32.lib', 'winmm.lib', 'ws2_32.lib', 'strmiids.lib', 'd3d11.lib', 'gdi32.lib', 'dxgi.lib', 'dwmapi.lib',
    'shcore.lib', 'bcrypt.lib', 'user32.lib', 'advapi32.lib')
  & cl.exe /nologo /std:c++20 /EHsc /MT /O1 /W0 /bigobj @defs "/I$A\include" "/I$A\include\third_party\abseil-cpp" `
    "/I$A\include\third_party\libyuv\include" "$here\tinhat_tx_test.cc" "/Fo$t\" "/Fe$t\tinhat_tx_test.exe" `
    /link "/LIBPATH:$A\lib" @libs | Out-Host
  if ($LASTEXITCODE -ne 0) { Write-Output "harness: compile failed (cl exit $LASTEXITCODE)"; exit 2 }
  $runArgs = @('--vectors', (Join-Path $repo 'testvectors\tx-len\vectors.json'))
  if ($v) { $runArgs += '-v' }
  & "$t\tinhat_tx_test.exe" @runArgs
  exit $LASTEXITCODE
} finally {
  Remove-Item -Recurse -Force $t -ErrorAction SilentlyContinue
}
