import yaml


PACKAGE_NAME_TO_TEST = {"boltons": "my-boltons-name", "jupyter-ros": "my-name-from-mapping"}


if __name__ == "__main__":
    # this will test if we map correctly our packages
    # we have one remote mapping for conda-forge
    # and one local mapping for robostack

    with open("pixi.lock") as pixi_lock:
        lock = yaml.safe_load(pixi_lock)

    expected_packages = [
        package for package in lock["packages"] if package["name"] in PACKAGE_NAME_TO_TEST
    ]

    assert len(expected_packages) == 2

    for package in expected_packages:
        package_name = package["name"]
        purls = package["purls"]

        # we have only one name in mapping
        # so purls also should be only one
        assert len(purls) == 1

        expected_purl = f"pkg:pypi/{PACKAGE_NAME_TO_TEST[package_name]}"

        assert purls[0] == expected_purl
