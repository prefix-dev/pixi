import os
import subprocess

RTD = "READTHEDOCS"

if os.getenv(RTD) == "True":
    root_doc = "rtd"

    def setup(app) -> None:
        """Customize the sphinx build lifecycle."""

        def _run_pixi(*_args) -> None:
            args = ["pixi", "run", "-e", "rtd", "rtd"]
            env = {k: v for k, v in os.environ.items() if k != RTD}
            subprocess.check_call(args, env=env)

        app.connect("build-finished", _run_pixi)
else:
    # exclude RTD
    exclude_patterns = ["rtd.rst"]

    # the "real" configuration goes here...
    extensions = ["myst_parser"]

# ... RTD will add additional configuration here
