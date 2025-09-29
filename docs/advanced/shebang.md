!!!warning "Only on Unix-like systems"
    The following approach only works on Unix-like systems (i.e. Linux and macOS) since Windows does not support shebang lines.

For simple scripts, you can use [`pixi exec`](../reference/cli/pixi/exec.md) to run them directly without needing to take care of installing dependencies or setting up an environment.
This can be done by adding a [shebang line](https://en.wikipedia.org/wiki/Shebang_(Unix)) at the top of the script, which tells the system how to execute the script.
Usually, a shebang line starts with `#!/usr/bin/env` followed by the name of the interpreter to use.

Instead of adding an interpreter, you can also just add `pixi exec` at the beginning of the script.
The only requirement for your script is that you must have `pixi` installed on your system.

!!!tip "Making the script executable"
    You might need to make the script executable by running `chmod +x my-script.sh`.

```bash title="use-bat.sh"
#!/usr/bin/env -S pixi exec --spec bat -- bash -e

bat my-file.json
```

!!!info "Explanation what's happening"
    The `#!` are magic characters that tell your system how to execute the script (take everything after the `#!` and append the file name).

    The `/usr/bin/env` is used to find `pixi` in the system's `PATH`.
    The `-S` option tells `/usr/bin/env` to use the first argument as the interpreter and the rest as arguments to the interpreter.
    `pixi exec --spec bat` creates a temporary environment containing only [`bat`](https://github.com/sharkdp/bat).
    `bash -e` (separated with `--`) is the command that is executed in this environment.
    So in total, `pixi exec --spec bat -- bash -e use-bat.sh` is being executed when you run `./use-bat.sh`.

You can also write self-contained python files that ship with their dependencies.
This example shows a very simple CLI that installs a Pixi environment to an arbitrary prefix using [`py-rattler`](https://conda.github.io/rattler/py-rattler) and [`typer`](https://typer.tiangolo.com).

```python title="install-pixi-environment-to-prefix.py"
#!/usr/bin/env -S pixi exec --spec py-rattler>=0.10.0,<0.11 --spec typer>=0.15.0,<0.16 -- python

import asyncio
from pathlib import Path
from typing import get_args

from rattler import install as rattler_install
from rattler import LockFile, Platform
from rattler.platform.platform import PlatformLiteral
from rattler.networking import Client, MirrorMiddleware, AuthenticationMiddleware
import typer


app = typer.Typer()


async def _install(
    lock_file_path: Path,
    environment_name: str,
    platform: Platform,
    target_prefix: Path,
) -> None:
    lock_file = LockFile.from_path(lock_file_path)
    environment = lock_file.environment(environment_name)
    if environment is None:
        raise ValueError(f"Environment {environment_name} not found in lock file {lock_file_path}")
    records = environment.conda_repodata_records_for_platform(platform)
    if not records:
        raise ValueError(f"No records found for platform {platform} in lock file {lock_file_path}")
    await rattler_install(
        records=records,
        target_prefix=target_prefix,
        client=Client(
            middlewares=[
                MirrorMiddleware({"https://conda.anaconda.org/conda-forge": ["https://repo.prefix.dev/conda-forge"]}),
                AuthenticationMiddleware(),
            ]
        ),
    )


@app.command()
def install(
    lock_file_path: Path = Path("pixi.lock").absolute(),
    environment_name: str = "default",
    platform: str = str(Platform.current()),
    target_prefix: Path = Path("env").absolute(),
) -> None:
    """
    Installs a pixi.lock file to a custom prefix.
    """
    if platform not in get_args(PlatformLiteral):
        raise ValueError(f"Invalid platform {platform}. Must be one of {get_args(PlatformLiteral)}")
    asyncio.run(
        _install(
            lock_file_path=lock_file_path,
            environment_name=environment_name,
            platform=Platform(platform),
            target_prefix=target_prefix,
        )
    )


if __name__ == "__main__":
    app()
```
