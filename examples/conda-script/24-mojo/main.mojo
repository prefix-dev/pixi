# /// conda-script
# channels = ["https://conda.modular.com/max", "conda-forge"]
# entrypoint = "mojo ${SCRIPT}"
#
# [dependencies]
# mojo = "*"
# numpy = "*"
# /// end-conda-script

from std.python import Python


def main() raises:
    var np = Python.import_module("numpy")
    var a = np.arange(1, 7)
    print("sum:", a.sum())
    print("mean:", a.mean())
