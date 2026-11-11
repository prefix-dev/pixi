# /// conda-script
# channels = ["https://prefix.dev/conda-forge"]
# entrypoint = "python ${SCRIPT}"
#
# [dependencies]
# python = "*"
# pyyaml = "*"
# /// end-conda-script
import yaml

document = yaml.safe_load(
    """
name: conda-script
languages:
  - python
  - c
"""
)
document["count"] = len(document["languages"])
print(yaml.safe_dump(document, sort_keys=True), end="")
