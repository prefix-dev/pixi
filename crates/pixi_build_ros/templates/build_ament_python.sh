set -eo pipefail

# Rattler-build will not set the SRC_DIR anymore so we set it through templating
export SRC_DIR="@SRC_DIR@"

pushd $SRC_DIR

# If there is a setup.cfg that contains install-scripts then we should not set it here
if [ -f setup.cfg ] && grep -q "install[-_]scripts" setup.cfg; then
    # Remove e.g. ros-humble- from PKG_NAME
    PKG_NAME_SHORT=${PKG_NAME#*ros-@DISTRO@-}
    # Substitute "-" with "_"
    PKG_NAME_SHORT=${PKG_NAME_SHORT//-/_}
    INSTALL_SCRIPTS_ARG="--install-scripts=$PREFIX/lib/$PKG_NAME_SHORT"
    echo "WARNING: setup.cfg not set, will set INSTALL_SCRIPTS_ARG to: $INSTALL_SCRIPTS_ARG"
    # The prefix is reused, so record what this build installed instead of
    # letting rattler-build package the previous build's leftovers as well.
    $PYTHON setup.py install --prefix="$PREFIX" --install-lib="$SP_DIR" $INSTALL_SCRIPTS_ARG --single-version-externally-managed --record="$RATTLER_BUILD_PACKAGE_FILES"

    # Remove build artifacts from setup.py install
    rm -rf *.egg-info 2>/dev/null || true
    rm -rf build/ 2>/dev/null || true
else
    # pip uninstalls the previous install before reinstalling, so nothing is left
    # behind and rattler-build can keep deriving the contents from the prefix.
    $PYTHON -m pip install . --no-deps --force-reinstall -vvv
fi
