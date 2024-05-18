# ReadTheDocs

!!! hint "Why ReadTheDocs?"

    [ReadTheDocs] (RTD) provides building and hosting of documentation sites
    with features for documentation readers contributors, and maintainers,
    free for all open source projects:

    - a custom subdomain on `.readthedocs.org` and `.rtfd.io`, or a custom domain
    - per-push pull request preview sites
    - per-version or -branch subfolders
    - translation support
    - automatic version switcher
    - automatic version aliases like `/latest/` and `/stable/`
    - privacy-respecting usage and search analytics

[ReadTheDocs]: https://readthdocs.com

While RTD cannot use `pixi` directly, the `.readthedocs.yaml`
configuration file is flexible enough for [basic usage](#basic-usage) with
minimal configuration.

With a few workarounds, [advanced usage](#advanced-usage) for
more complex builds is possible.

## Basic Usage

!!! warning "Beta"

    This build [override] approach is in **beta**: see the latest RTD documentation.

Using the full build process override, RTD provides only:

- an Ubuntu container
- a working `mamba` to bootstrap a specific `pixi` version
- `$READTHEDOCS_OUTPUT`, an environment variable into which the site should be built

It is up to the project's [build.commands] to do the rest.

[build.commands]: https://docs.readthedocs.io/en/stable/config-file/v2.html#build-commands


### Example Project

Consider a minimal [`mkdocs`][mkdocs]-based project, which defines its `pixi`
manifest in `pyproject.toml`:

```
my-basic-project/
 ├─ pyproject.toml
 ├─ mkdocs.yml
 └─ docs/
    └─ index.md
```

### Import Your Project

Import the version control project via the ReadTheDocs web interface, as
described in the [tutorial] or [import guide].

!!! tip "Pull Request Builds"

    Enable [pull requests builds] to get a custom subdomain per PR,
    updated on each push. This will appear as a continuous integration check,
    with a link to the documentation.

[pull requests builds]: https://docs.readthedocs.io/en/stable/guides/pull-requests.html
[tutorial]: https://docs.readthedocs.io/en/stable/tutorial/index.html#getting-started
[import guide]: https://docs.readthedocs.io/en/stable/intro/import-guide.html
[override]: https://docs.readthedocs.io/en/stable/build-customization.html#override-the-build-process
[mkdocs]: https://www.mkdocs.org
[sphinx]: https://www.sphinx-doc.org

### Update the `pixi` manifest

Create a `readthedocs` task in `pyproject.toml`:

```toml
# pyproject.toml
[tool.pixi.tasks]
readthedocs = "mkdocs build && cp -r site $READTHEDOCS_OUTPUT/html"
```

### Override the RTD Build Process in YAML

Create the well-known `.readthedocs.yaml`:

```yaml
# .readthedocs.yaml
version: 2
build:
  os: ubuntu-22.04
  tools:
    python: mambaforge-latest  # this ensures a viable `mamba`
  commands:
    - mamba install -c conda-forge -c nodefaults pixi==0.22.0
    - pixi run readthedocs
```

!!! tip "Custom YAML Location"

    While the filename can't be changed, `.readthedocs.yaml` can be placed in
    another folder, such as `docs`, to keep the project root more tidy.

Commit, push, and observe the builds in the RTD project dashboard, which
includes links to the built site.

### Limitations

- none of the RTD-specific features provided by e.g. [readthedocs-sphinx-ext]
- no custom debian packages [build.apt_packages] can be installed

[readthedocs-sphinx-ext]: https://github.com/readthedocs/readthedocs-sphinx-ext
[build.apt_packages]: https://docs.readthedocs.io/en/stable/config-file/v2.html#build-apt-packages

## Advanced Usage

Consider a project which uses a `firefox` browser to take screenshots of a working
web application, included in a [sphinx] build process.

Such a documentation pipeline may have platform-specific requirements which
can't be provided by a `pixi` environment, or require specific dependencies in the
RTD-provided container.

In this case, isolating RTD-specific configuration in a dedicated feature/environment
can make these complications more bearable.

### Challenges

An unsatisfying experience with `pixi` on RTD might go something like this:

> While [`firefox`][firefox-feedstock] is available from `conda-forge`, when trying to
> use this in RTD, the build complains about missing binary packages, not yet built
> for `conda-forge`.

> Deciphering the `.so` names, and installing the correct debian
> packages with [build.apt_packages], this has no effect in the above workflow,
> as [build.commands] will be silently ignored.

> When going to the supported build [extension] method, trying to install `firefox`
> with `build.apt_packages` installs without error, but fails during the build.

> Logs reveal this installs a `snap`... but `snapd` is _also_ not installed, and
> can't be enabled!

### Workarounds

- include an LTS version of `firefox` in a `pixi` environment
- install `firefox`'s binary dependencies with [build.apt_packages]
- use the RTD [build.jobs] extension mechanism
  - in `build.jobs.pre_build` install `pixi`, the `rtd` environment, and run the
    _real_ documentation build
  - let the default builder do its job against a _fake_ documentation root
  - in `build.jobs.post_build`, overwrite the well-known location with the _real_
    documentation

[build.jobs]: https://docs.readthedocs.io/en/stable/config-file/v2.html#build-jobs

### Repo Structure

```
my-advanced-project/
 ├─ pixi.toml
 ├─ _scripts/
 │  └─ fake-docs/
 │     ├─ conf.py           # this can be empty
 │     └─ index.rst         # this needs at least one h1 heading
 └─ docs/
    ├─ .readthedocs.yaml    # location customized via RTD web UI
    ├─ conf.py
    └─ environment.yml      # isolate the pixi version
```

[firefox-feedstock]: https://github.com/conda-forge/firefox-feedstock
[extension]: https://docs.readthedocs.io/en/stable/build-customization.html#extend-the-build-process

### Update the `pixi` manifest

In this example, `feature`s are used to better isolate dependencies and tasks.

```toml
# pixi.toml
[feature.docs.tasks.docs]
cmd = "sphinx-build -W --keep-going --color -b html docs ./build/docs"

[feature.rtd.tasks.fake-docs-post-build]
cmd = "rm -rf $READTHEDOCS_OUTPUT/html && cp -r build/docs $READTHEDOCS_OUTPUT/html"

# dependencies
[environments]
rtd = ["rtd", "docs"]
docs = ["docs"]

[feature.docs.dependencies]
sphinx = "*"

[feature.rtd]
platforms = ["linux-64"]
dependencies = { firefox = "115.*" }
```

### Describe the RTD Bootstrap Environment

Using a dedicated file to describe the [conda.environment] into which a known
version of `pixi` should be installed keeps this concern out of an already-complex
RTD configuration.

[conda.environment]: https://docs.readthedocs.io/en/stable/config-file/v2.html#conda-environment

```yaml
channels:
  - conda-forge
  - nodefaults
dependencies:
  - pixi ==0.22.0
```

### Extend the RTD Build Process in YAML

```yaml
version: 2
build:
  os: ubuntu-22.04
  apt_packages:
    - libasound2
    - libatk1.0-0
    - libcups2
    - libdbus-glib-1-2
    - libgtk-3-0
    - libnss3
    - libpangocairo-1.0-0
    - libx11-xcb1
    - libxcomposite1
    - libxcursor1
    - libxdamage1
    - libxi6
    - libxrandr2
    - libxss1
    - libxtst6
  tools:
    python: mambaforge-latest
  jobs:
    pre_build:
      - pixi install --environment=rtd
      - pixi run --environment=rtd docs
    post_build:
      - pixi run --environment=rtd fake-docs-post-build
sphinx:    # this doesn't matter, but must be configured
  builder: html
  configuration: _scripts/fake-docs/conf.py
conda:
  environment: docs/environment.yml
```

### Limitations

- as with the basic approach, no special RTD features will be enabled
