# ReadTheDocs: Extend Build

This example shows how to [extend] a [ReadTheDocs] `sphinx` build by customizing:
- `docs/.readthedocs.yaml` to:
  - install native `apt` packages to support a heavyweight dependency
  - bootstrap a [conda.environment] with the supported `mambaforge`
  - prepare a `pixi` environment with [build.jobs]
  - avoid another file in the project root
    - this requires manual configuration in the ReadTheDocs web UI
- `docs/conf.py` to:
  - support extended ReadTheDocs features provided by [readthedocs-sphinx-ext]
  - allow the default build to run against an un-published `rtd.rst`
  - use `sphinx` lifecycle events to run the actual build with `pixi` tasks

> [!NOTE]
>
> For a simpler `mkdocs` build, see the [`readthedocs-override`][override] example.

[ReadTheDocs]: https://readthdocs.com
[extend]: https://docs.readthedocs.io/en/stable/build-customization.html#extend-the-build-process
[build.jobs]: https://docs.readthedocs.io/en/stable/config-file/v2.html#build-jobs
[readthedocs-sphinx-ext]: https://github.com/readthedocs/readthedocs-sphinx-ext
[conda.environment]: https://docs.readthedocs.io/en/stable/config-file/v2.html#conda-environment
[override]: ../readthedocs-override/README.md
