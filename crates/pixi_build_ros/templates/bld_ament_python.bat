setlocal

set "PYTHONPATH=%LIBRARY_PREFIX%\lib\site-packages;%SP_DIR%"

:: Rattler-build will not set the SRC_DIR anymore so we set it through templating
set "SRC_DIR=@SRC_DIR@"

:: `setup.py install --record` resolves a relative path against the current
:: directory, which becomes the source tree below. Record into the work directory
:: instead so the build leaves nothing behind in the source tree.
set "RECORD_FILE=%CD%\files.txt"

pushd %SRC_DIR%
set "PKG_NAME_SHORT=%PKG_NAME:*ros-@DISTRO@-=%"
set "PKG_NAME_SHORT=%PKG_NAME_SHORT:-=_%"

:: If there is a setup.cfg that contains install-scripts then use pip to install
findstr install[-_]scripts setup.cfg
if "%errorlevel%" == "0" (
    %PYTHON% setup.py install --single-version-externally-managed --record="%RECORD_FILE%" ^
        --prefix=%LIBRARY_PREFIX% ^
        --install-lib=%SP_DIR% ^
         --install-scripts=%LIBRARY_PREFIX%\lib\%PKG_NAME_SHORT%
) else (
    %PYTHON% setup.py install --single-version-externally-managed --record="%RECORD_FILE%" ^
        --prefix=%LIBRARY_PREFIX% ^
        --install-lib=%SP_DIR% ^
        --install-scripts=%LIBRARY_PREFIX%\bin
)
if errorlevel 1 exit 1

:: `setup.py install` only copies files, it never removes them, so a prefix
:: reused by an incremental build still holds the files of the previous build.
:: Hand the recorded list to rattler-build so the package contains what this
:: build installed instead of everything found in the prefix. Entries that do
:: not exist are skipped because rattler-build rejects them.
>>"%RATTLER_BUILD_PACKAGE_FILES%" (
    for /F "usebackq delims=" %%F in ("%RECORD_FILE%") do if exist "%%F" echo %%F
)
del /q "%RECORD_FILE%"

:: Cleanup build artifacts
for /d %%d in (*.egg-info) do rmdir /s /q "%%d" 2>nul
if exist build rmdir /s /q build 2>nul

if errorlevel 1 exit 1
