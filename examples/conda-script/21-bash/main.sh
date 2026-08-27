# /// conda-script
# channels = ["conda-forge"]
# entrypoint = "brush ${SCRIPT}"
#
# [dependencies]
# brush = "*"
# jq = "*"
# /// end-conda-script
set -euo pipefail

languages='{"name": "conda-script", "languages": ["python", "c", "bash"]}'

printf '%s\n' "$languages" | jq --raw-output '.languages[]'
printf '%s\n' "$languages" | jq '{name, count: (.languages | length)}'
