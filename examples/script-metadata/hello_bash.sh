#!/bin/bash
# /// script
# [tool.conda]
# channels = ["conda-forge"]
# dependencies = ["bash 5.*", "curl", "jq"]
#
# [tool.pixi]
# entrypoint = "bash"
# ///

# A simple Hello World bash script with inline metadata.
# Run with: pixi exec hello_bash.sh

echo "========================================"
echo "Hello from Bash with inline script metadata!"
echo "========================================"
echo "Bash version: $BASH_VERSION"
echo "Platform: $(uname -s) $(uname -m)"
echo ""

# Demonstrate installed tools
echo "Installed tools:"
echo "- curl version: $(curl --version | head -n1)"
echo "  - installed at: $(which curl)"
echo "- jq version: $(jq --version)"
echo "  - installed at: $(which jq)"
echo "========================================"
