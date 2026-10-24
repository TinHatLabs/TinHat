@echo off
rem Patched-libwebrtc build (Windows) for the AV1-E2EE fix. Derived from
rem build_windows.cmd at tag webrtc-51ef663: patches are NOT applied here
rem (applied once, by hand, so this is idempotent), and the patched cryptor
rem object is compiled first so a syntax error fails in seconds, not hours.
setlocal enabledelayedexpansion
cd /d C:\webrtc\libwebrtc
set PATH=C:\webrtc\libwebrtc\depot_tools;%PATH%
set DEPOT_TOOLS_WIN_TOOLCHAIN=0
set GYP_GENERATORS=ninja,msvs-ninja
set GYP_MSVS_VERSION=2022
set OUTPUT_DIR=src\out-x64-release
set ARTIFACTS_DIR=C:\webrtc\libwebrtc\win-x64-release
rem WEBRTC_ARTIFACTS_DIR writes the library beside an existing one instead of over it.
if defined WEBRTC_ARTIFACTS_DIR set "ARTIFACTS_DIR=%WEBRTC_ARTIFACTS_DIR%"
for %%I in ("%ARTIFACTS_DIR%\.") do set "ARTIFACTS_DIR=%%~fI"
rem Its include folder is deleted and rebuilt below, so it must be a folder of its own:
rem not inside src or depot_tools, and not the recipe folder or one of its parents.
if "!ARTIFACTS_DIR:~-1!"=="\" (set "ART_CHECK=!ARTIFACTS_DIR!") else (set "ART_CHECK=!ARTIFACTS_DIR!\")
set "RECIPE_CHECK=C:\webrtc\libwebrtc\"
if /i not "!ART_CHECK:C:\webrtc\libwebrtc\src\=!"=="!ART_CHECK!" ( echo FATAL: the artifact folder must not be inside the source tree & exit /b 1 )
if /i not "!ART_CHECK:C:\webrtc\libwebrtc\depot_tools\=!"=="!ART_CHECK!" ( echo FATAL: the artifact folder must not be inside depot_tools & exit /b 1 )
if /i not "!RECIPE_CHECK:%ART_CHECK%=!"=="!RECIPE_CHECK!" ( echo FATAL: the artifact folder must not be the recipe folder or one of its parents & exit /b 1 )
set "vs2022_install=C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools"

copy /y "src\.vpython3" "." >nul

if not exist "%ARTIFACTS_DIR%\lib" mkdir "%ARTIFACTS_DIR%\lib"

rem args verbatim from build_windows.cmd (= the win prebuilt's args.gn)
call gn.bat gen %OUTPUT_DIR% --root="src" --args="is_debug=false is_clang=true target_cpu=\"x64\" use_custom_libcxx=false rtc_libvpx_build_vp9=true enable_libaom=true rtc_include_tests=false rtc_build_examples=false rtc_build_tools=false is_component_build=false rtc_enable_protobuf=false rtc_use_h264=true ffmpeg_branding=\"Chrome\" symbol_level=0 enable_iterator_debugging=false"
if errorlevel 1 exit /b 1

set CRYPTOR=
for /f "tokens=1 delims=:" %%t in ('ninja.exe -C %OUTPUT_DIR% -t targets all ^| findstr /c:"frame_crypto_transformer.o"') do if not defined CRYPTOR set "CRYPTOR=%%t"
if not defined CRYPTOR (
  echo FATAL: no ninja target for frame_crypto_transformer object
  exit /b 1
)
echo === fail-fast: building !CRYPTOR! ===
ninja.exe -C %OUTPUT_DIR% "!CRYPTOR!"
if errorlevel 1 exit /b 1

set DAV1DDEC=
for /f "tokens=1 delims=:" %%t in ('ninja.exe -C %OUTPUT_DIR% -t targets all ^| findstr /c:"dav1d_decoder.o"') do if not defined DAV1DDEC set "DAV1DDEC=%%t"
if not defined DAV1DDEC (
  echo FATAL: no ninja target for dav1d_decoder object
  exit /b 1
)
echo === fail-fast: building !DAV1DDEC! ===
ninja.exe -C %OUTPUT_DIR% "!DAV1DDEC!"
if errorlevel 1 exit /b 1

echo === full build ===
ninja.exe -C %OUTPUT_DIR% :default
if errorlevel 1 exit /b 1

echo === assembling artifacts ===
if exist "%ARTIFACTS_DIR%\lib\webrtc.lib" del /f /q "%ARTIFACTS_DIR%\lib\webrtc.lib"
copy /y "%OUTPUT_DIR%\obj\webrtc.lib" "%ARTIFACTS_DIR%\lib"
if errorlevel 1 ( echo FATAL: could not copy webrtc.lib & exit /b 1 )
rem Both patches carry a grep-able stamp; a library without them was built from
rem an unpatched or older tree and must never be linked.
for %%s in (tinhat-av1-e2ee-v2 tinhat-dav1d-fp-v1) do (
  findstr /m /c:"%%s" "%ARTIFACTS_DIR%\lib\webrtc.lib" >nul
  if errorlevel 1 (
    echo FATAL: stamp %%s missing from "%ARTIFACTS_DIR%\lib\webrtc.lib" - patch not applied?
    exit /b 1
  )
  echo stamp OK: %%s
)
copy /y "%OUTPUT_DIR%\obj\webrtc.ninja" "%ARTIFACTS_DIR%"
copy /y "%OUTPUT_DIR%\obj\modules\desktop_capture\desktop_capture.ninja" "%ARTIFACTS_DIR%"
copy /y "%OUTPUT_DIR%\args.gn" "%ARTIFACTS_DIR%"

if exist "%ARTIFACTS_DIR%\include" rmdir /s /q "%ARTIFACTS_DIR%\include"
xcopy src\*.h "%ARTIFACTS_DIR%\include" /C /S /I /H /Y >nul
xcopy src\*.inc "%ARTIFACTS_DIR%\include" /C /S /I /H /Y >nul
echo === DONE: %ARTIFACTS_DIR% ===
