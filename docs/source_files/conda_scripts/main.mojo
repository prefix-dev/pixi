# /// conda-script
# channels = [
#     "https://prefix.dev/modular-community",
#     "https://conda.modular.com/max",
#     "https://prefix.dev/conda-forge",
# ]
# entrypoint = "mojo ${SCRIPT}"
#
# [dependencies]
# mojo = "*"
# emberjson = "*"
# /// end-conda-script

from emberjson import parse, to_string


def main() raises:
    var document = parse(
        '{"name": "conda-script", "languages": ["mojo", "python"], "count": 2}'
    )
    ref languages = document.object()["languages"].array()
    print("languages:", len(languages), "first:", languages[0].string())
    print(to_string[pretty=True](document))
