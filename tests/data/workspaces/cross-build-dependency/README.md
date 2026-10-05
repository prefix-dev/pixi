Workspace for the cross-compilation regression test of
<https://github.com/prefix-dev/pixi/issues/6846>.

`package_a` depends on the source package `package_b` in both its build and
its host section, the shape the ROS backend produces for every `package.xml`
dependency. `package_b` has a binary host dependency (`dummy-b`, from the
`dummy_channel_1` test channel) so the platform its host environment is
solved for is observable. Both packages produce platform-specific outputs.
