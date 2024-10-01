let platforms = ["win-64", "linux-64", "osx-arm64", "osx-64"]
let channels = [$"($env.FILE_PWD)/dummy_channel_1", $"($env.FILE_PWD)/dummy_channel_2", , $"($env.FILE_PWD)/non_self_expose_channel"]
for channel in $channels {
    cd $channel
    rm -rf output
    for platform in $platforms {
        rattler-build build --target-platform $platform
    }
    rm -rf output/bld
}
