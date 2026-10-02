!!! warning "Variable references are not expanded by pixi"
    Values are passed to the build shell unchanged, so a reference like `$PREFIX` is only resolved if that shell understands it.
    Linux and macOS builds run through `bash`, which expands `$PREFIX`.
    Windows builds run through `cmd.exe`, which expands `%PREFIX%` and leaves `$PREFIX` as literal text.

    Use target-specific configuration when you need both:

    ```toml
    [package.build.target.unix.config]
    env = { MY_INCLUDE_DIR = "$PREFIX/include" }

    [package.build.target.win.config]
    env = { MY_INCLUDE_DIR = "%PREFIX%\\Library\\include" }
    ```

    `${{ PREFIX }}` is not an alternative here.
    Templates in `env` are rendered while the recipe is evaluated, which happens before the build prefix exists.
    They only work in build scripts.
