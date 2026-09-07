#!/bin/bash
# This file contains shell command examples for package specifications documentation

# --8<-- [start:quick-add-examples]
# Install a specific version
pixi add python=3.11

# Install with version constraints
pixi add "numpy>=1.21,<2.0"

# Install a specific build (e.g., CUDA-enabled package) using = syntax
pixi add "pytorch=*=cuda*"

# Alternative bracket syntax for build specification
pixi add "pytorch [build='cuda*']"

# Specify both version and build using bracket syntax
pixi add "pytorch [version='2.9.*', build='cuda*']"

# Simple PyPI package
pixi add --pypi requests

# PyPI package version range
pixi add --pypi "requests>=2.20,<3.0"

# PyPI package with extras
pixi add --pypi "requests[security]==2.25.1"
# --8<-- [end:quick-add-examples]

# --8<-- [start:quick-global-examples]
# Install a specific version
pixi global install python=3.11

# Install with version constraints
pixi global install "numpy>=1.21,<2.0"

# Install a specific build (e.g., CUDA-enabled package) using = syntax
pixi global install "pytorch=*=cuda*"

# Alternative bracket syntax for build specification
pixi global install "pytorch [build='cuda*']"

# Specify both version and build using bracket syntax
pixi global install "pytorch [version='2.9.*', build='cuda*']"
# --8<-- [end:quick-global-examples]

# --8<-- [start:quick-exec-examples]
# Execute a command in an ephemeral environment
pixi exec python

# Execute with specific package versions
pixi exec -s python=3.11 python

# Execute with specific package builds
pixi exec -s "python=*=*cp313" python

# Execute with channel specification
pixi exec --channel conda-forge python
# --8<-- [end:quick-exec-examples]
