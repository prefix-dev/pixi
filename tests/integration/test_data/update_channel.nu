export def main [channel: string] {
    let platforms = ["win-64", "linux-64", "osx-arm64", "osx-64"]
    cd $channel
    rm -rf output
    for platform in $platforms {
        rattler-build build --target-platform $platform
    }
    rm -rf output/bld
}
