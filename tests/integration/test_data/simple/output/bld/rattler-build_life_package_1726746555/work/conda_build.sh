
## Start of bash preamble
if [ -z ${CONDA_BUILD+x} ]; then
    source /var/home/julian/Projekte/github.com/prefix-dev/pixi-1/tests/integration/test_data/simple/output/bld/rattler-build_life_package_1726746555/work/build_env.sh
fi
# enable debug mode for the rest of the script
set -x
## End of preamble

mkdir -p $PREFIX/bin
cp life.v127.com $PREFIX/bin/life.com
chmod +x $PREFIX/life.com
