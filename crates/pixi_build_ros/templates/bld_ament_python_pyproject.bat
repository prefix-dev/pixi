:: Build script for ament_python packages that use pyproject.toml instead of setup.py.
setlocal

set "PYTHONPATH=%LIBRARY_PREFIX%\lib\site-packages;%SP_DIR%"

:: Rattler-build will not set the SRC_DIR anymore so we set it through templating
set "SRC_DIR=@SRC_DIR@"

pushd %SRC_DIR%

:: Build with the build backend from the host environment (no network at build time)
%PYTHON% -m pip install . --no-deps --no-build-isolation -vvv
if errorlevel 1 exit 1

:: Register the package in the ament resource index. setup.py packages do this
:: via data_files; pyproject-only packages cannot express it.
if not exist "%LIBRARY_PREFIX%\share\ament_index\resource_index\packages" mkdir "%LIBRARY_PREFIX%\share\ament_index\resource_index\packages"
type nul > "%LIBRARY_PREFIX%\share\ament_index\resource_index\packages\@ROS_PKG_NAME@"
if not exist "%LIBRARY_PREFIX%\share\@ROS_PKG_NAME@" mkdir "%LIBRARY_PREFIX%\share\@ROS_PKG_NAME@"
copy package.xml "%LIBRARY_PREFIX%\share\@ROS_PKG_NAME@\package.xml"
if errorlevel 1 exit 1

@ENTRY_POINT_INSTALL@

if errorlevel 1 exit 1
