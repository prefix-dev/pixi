# Making a release of pixi

## Prep
- Make sure main is up-to-date and ci passes: [![build-badge](https://img.shields.io/github/actions/workflow/status/prefix-dev/pixi/rust.yml?style=flat-square&branch=main)](https://github.com/prefix-dev/pixi/actions/workflows/rust.yml?query=branch%3Amain+)
- Set the variable `export RELEASE_VERSION=X.Y.Z` in your shell
- Make a new branch for the release: `git checkout main && git pull upstream main && git checkout -b bump/prepare-v$RELEASE_VERSION`
- Bump all versions: `pixi run bump`
- Update the changelog: `pixi run bump-changelog`
  - Don't forget to update the "Highlights" section.
- Commit the changes: `git commit -am "chore: version to $RELEASE_VERSION"`
- Push the changes: `git push origin`

## Release prep PR
- Create a PR to check off the change with the peers
- Merge that PR

## Tagging the release
- Checkout main: `git fetch && git checkout upstream/main`
- Tag the release: `git tag v$RELEASE_VERSION -m "Release $RELEASE_VERSION"`
- Push the tag: `git push upstream v$RELEASE_VERSION`

## Publishing the release
- After that, update the Release which has CI created for you (after the first build) and add the changelog to the release notes.
- Make sure all the artifacts are there and the CI is green!!!
- Publish the release and make sure it is set as latest.

## Test the release using the install script:
- `curl -fsSL https://pixi.sh/install.sh | bash` or `iwr -useb https://pixi.sh/install.ps1 | iex`
- `pixi --version` should show the new version

DONE!
