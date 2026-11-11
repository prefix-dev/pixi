from .read_wheels import Package, PackageSpec, WheelTest


def test_spec_to_add_cmd() -> None:
    assert Package("foo", PackageSpec()).to_add_cmd() == "foo"
    assert Package("foo", PackageSpec("1.0")).to_add_cmd() == "foo==1.0"
    assert Package("foo", PackageSpec("1.0", "bar")).to_add_cmd() == "foo[bar]==1.0"


def test_wheel_test_from_str() -> None:
    toml = """
    foo = "*"
    bar = { version = "1.0", extras = "baz", target = "linux-64" }
    laz = ["*", { version = "1.0", extras = "baz" }]
    """
    wt = WheelTest.from_str(toml)
    assert len(list(wt.to_packages())) == 4
    assert Package("foo", PackageSpec()) in wt.to_packages()
    assert Package("bar", PackageSpec("1.0", "baz", "linux-64")) in wt.to_packages()
    assert Package("laz", PackageSpec()) in wt.to_packages()
    assert Package("laz", PackageSpec("1.0", "baz")) in wt.to_packages()
