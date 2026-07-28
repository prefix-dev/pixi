setlocal

set "PYTHONPATH=%LIBRARY_PREFIX%\lib\site-packages;%SP_DIR%"

:: Rattler-build will not set the SRC_DIR anymore so we set it through templating
set "SRC_DIR=@SRC_DIR@"

pushd %SRC_DIR%
set "PKG_NAME_SHORT=%PKG_NAME:*ros-@DISTRO@-=%"
set "PKG_NAME_SHORT=%PKG_NAME_SHORT:-=_%"

:: The prefix is reused, so record what this build installed instead of letting
:: rattler-build package the previous build's leftovers as well.
:: `--no-compile`: setuptools records .pyc files it does not always write.

:: If there is a setup.cfg that contains install-scripts then use pip to install
findstr install[-_]scripts setup.cfg
if "%errorlevel%" == "0" (
    %PYTHON% setup.py install --single-version-externally-managed --no-compile --record="%RATTLER_BUILD_PACKAGE_FILES%" ^
        --prefix=%LIBRARY_PREFIX% ^
        --install-lib=%SP_DIR% ^
         --install-scripts=%LIBRARY_PREFIX%\lib\%PKG_NAME_SHORT%
) else (
    %PYTHON% setup.py install --single-version-externally-managed --no-compile --record="%RATTLER_BUILD_PACKAGE_FILES%" ^
        --prefix=%LIBRARY_PREFIX% ^
        --install-lib=%SP_DIR% ^
        --install-scripts=%LIBRARY_PREFIX%\bin
)
if errorlevel 1 exit 1

:: Cleanup build artifacts
for /d %%d in (*.egg-info) do rmdir /s /q "%%d" 2>nul
if exist build rmdir /s /q build 2>nul

if errorlevel 1 exit 1
