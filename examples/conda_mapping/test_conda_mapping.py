import yaml

# This test verify if we generate right purls for our packages
# We use one remote mapping for conda-forge channel
# and one local mapping for robostack channel


# For packages that are present in local-mapping
# we verify if source=project-defined-mapping qualifier is present in purl
# so purl should look like this:
# pkg:pypi/my-boltons-name?source=project-defined-mapping

PACKAGE_NAME_TO_TEST = {
    "boltons": "my-boltons-name?source=project-defined-mapping",
    "jupyter-ros": "my-name-from-mapping?source=project-defined-mapping",
}


# We test if having a null for conda name
# will mark a conda package as not a pypi package
# and will not add any purls for it
# "jupyter-amphion": null
PACKAGE_NAME_SHOULD_BE_NULL = ("jupyter-amphion",)


if __name__ == "__main__":
    with open("pixi.lock") as pixi_lock:
        lock = yaml.safe_load(pixi_lock)

    expected_packages = [
        package for package in lock["packages"] if package["name"] in PACKAGE_NAME_TO_TEST
    ]

    expected_null_packages = [
        package for package in lock["packages"] if package["name"] in PACKAGE_NAME_SHOULD_BE_NULL
    ]

    for package in expected_packages:
        package_name = package["name"]
        purls = package["purls"]

        # we have only one name in mapping
        # so purls also should be only one
        assert len(purls) == 1

        expected_purl = f"pkg:pypi/{PACKAGE_NAME_TO_TEST[package_name]}"

        assert purls[0] == expected_purl

    for package in expected_null_packages:
        # The package should have an empty list of purls
        assert package["purls"] == []
