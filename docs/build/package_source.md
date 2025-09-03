By default, the package definition assumes the location of the source to be in the root of the package definition.

For example if your package has the following structure:
```text
my_package
├── pixi.toml
├── src
│   └── my_code.cpp
└── include
    └── my_code.h
```
The source, Pixi assumes, is everything in the `my_package/` directory.

Otherwise, you can use on of the out of tree options Pixi provides to specify where the source is located.

## Path
If your source is located somewhere else, you can specify the location of the source using the `package.build.source.path` field.

For example if your package has the following structure:
```text
my_package
├── pixi.toml
└── source
    ├── src
    │   └── my_code.cpp
    └── include
        └── my_code.h
```
You can specify the location of the source like this:
```toml
[package.build.source]
path = "source"
```

This will also work with relative paths:
```toml
[package.build.source]
path = "../my_other_source_directory"
```

This works great in combination with git submodules.
