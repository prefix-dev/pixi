"""Update pixi-build-r's list of Bioconductor packages.

pixi-build-r names a dependency `bioconductor-<name>` rather than `r-<name>`
when it's a Bioconductor package, as bioconda does. This writes the names of
all Bioconductor packages (software, annotation data, experiment data and
workflows) of recent releases to
`crates/pixi_build_r/bioconductor_mapping/bioconductor_packages.json`, from
Bioconductor's own package lists. Older releases are included because
bioconda keeps their packages for older versions of R.

Usage: python scripts/update_bioconductor_packages.py [--oldest 3.18]
"""

import argparse
import json
import re
import urllib.request
from pathlib import Path

REPOSITORIES = ["bioc", "data/annotation", "data/experiment", "workflows"]
CONFIG_URL = "https://bioconductor.org/config.yaml"
PACKAGES_URL = "https://bioconductor.org/packages/{release}/{repository}/src/contrib/PACKAGES"
OUTPUT = (
    Path(__file__).parent.parent
    / "crates"
    / "pixi_build_r"
    / "bioconductor_mapping"
    / "bioconductor_packages.json"
)


def fetch(url: str) -> str:
    # bioconductor.org refuses urllib's default user agent
    request = urllib.request.Request(
        url, headers={"User-Agent": "pixi-update-bioconductor-packages"}
    )
    with urllib.request.urlopen(request) as response:
        return response.read().decode("utf-8")


def releases(oldest: str) -> list[str]:
    """The releases from `oldest` to Bioconductor's current release."""
    config = fetch(CONFIG_URL)
    current = re.search(r'^release_version:\s*"([\d.]+)"', config, re.MULTILINE)
    if current is None:
        raise SystemExit(f"Couldn't find release_version in {CONFIG_URL}")
    major, newest = (int(part) for part in current.group(1).split("."))
    oldest_major, first = (int(part) for part in oldest.split("."))
    if oldest_major != major:
        raise SystemExit(
            f"--oldest {oldest} isn't in the current major release {major}, which this doesn't support"
        )
    return [f"{major}.{minor}" for minor in range(first, newest + 1)]


def package_names(release: str, repository: str) -> set[str]:
    text = fetch(PACKAGES_URL.format(release=release, repository=repository))
    return set(re.findall(r"^Package:\s*(\S+)", text, re.MULTILINE))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--oldest", default="3.18", help="the oldest release to include")
    args = parser.parse_args()

    included = releases(args.oldest)
    names: set[str] = set()
    for release in included:
        for repository in REPOSITORIES:
            names |= package_names(release, repository)

    data = {
        "source": PACKAGES_URL.format(release="<release>", repository="<repository>"),
        "releases": included,
        "repositories": REPOSITORIES,
        "packages": sorted(names, key=str.lower),
    }
    OUTPUT.write_text(json.dumps(data, indent=2) + "\n")
    print(f"Wrote {len(names)} packages from Bioconductor {', '.join(included)} to {OUTPUT}")


if __name__ == "__main__":
    main()
