# Build script for ament_python packages that use pyproject.toml instead of setup.py.

set -eo pipefail

# Rattler-build will not set the SRC_DIR anymore so we set it through templating
export SRC_DIR="@SRC_DIR@"

pushd $SRC_DIR

# Build with the build backend from the host environment (no network at build time)
$PYTHON -m pip install . --no-deps --no-build-isolation -vvv

# Register the package in the ament resource index. setup.py packages do this
# via data_files; pyproject-only packages cannot express it.
mkdir -p "$PREFIX/share/ament_index/resource_index/packages"
touch "$PREFIX/share/ament_index/resource_index/packages/@ROS_PKG_NAME@"
mkdir -p "$PREFIX/share/@ROS_PKG_NAME@"
cp package.xml "$PREFIX/share/@ROS_PKG_NAME@/package.xml"

@ENTRY_POINT_INSTALL@
