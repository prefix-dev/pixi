# /// conda-script
# channels = ["conda-forge"]
# entrypoint = "python ${SCRIPT}"
#
# [dependencies]
# python = "3.13.*"
# simple-app = "0.1.*"
#
# [tool.pixi.dependencies]
# simple-app = { git = "https://github.com/prefix-dev/pixi-build-testsuite.git", subdirectory = "tests/data/pixi_build/minimal-backend-workspaces/pixi-build-python" }
# /// end-conda-script
"""A source dependency, built from git by pixi-build.

The binary spec in `[dependencies]` keeps the script solvable for tools that
only implement the conda-script specification, while the source spec under
`[tool.pixi.dependencies]` provides the candidate pixi actually builds. Both
specs reach the solver, so the version constrains the built package.
"""

import simple_app

simple_app.main()
