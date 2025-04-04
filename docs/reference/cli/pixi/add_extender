
--8<-- [start:example]

## Examples

```shell
pixi add numpy # (1)!
pixi add numpy pandas "pytorch>=1.8" # (2)!
pixi add "numpy>=1.22,<1.24" # (3)!
pixi add --manifest-path ~/myworkspace/pixi.toml numpy # (4)!
pixi add --host "python>=3.9.0" # (5)!
pixi add --build cmake # (6)!
pixi add --platform osx-64 clang # (7)!
pixi add --no-install numpy # (8)!
pixi add --no-lockfile-update numpy # (9)!
pixi add --feature featurex numpy # (10)!
pixi add --git https://github.com/wolfv/pixi-build-examples boost-check # (11)!
pixi add --git https://github.com/wolfv/pixi-build-examples --branch main --subdir boost-check boost-check # (12)!
pixi add --git https://github.com/wolfv/pixi-build-examples --tag v0.1.0 boost-check # (13)!
pixi add --git https://github.com/wolfv/pixi-build-examples --rev e50d4a1 boost-check # (14)!

# Add a pypi dependency
pixi add --pypi requests[security] # (15)!
pixi add --pypi Django==5.1rc1 # (16)!
pixi add --pypi "boltons>=24.0.0" --feature lint # (17)!
pixi add --pypi "boltons @ https://files.pythonhosted.org/packages/46/35/e50d4a115f93e2a3fbf52438435bb2efcf14c11d4fcd6bdcd77a6fc399c9/boltons-24.0.0-py3-none-any.whl" # (18)!
pixi add --pypi "exchangelib @ git+https://github.com/ecederstrand/exchangelib" # (19)!
pixi add --pypi "project @ file:///absolute/path/to/project" # (20)!
pixi add --pypi "project@file:///absolute/path/to/project" --editable # (21)!
pixi add --git https://github.com/mahmoud/boltons.git boltons --pypi # (22)!
pixi add --git https://github.com/mahmoud/boltons.git boltons --branch main --pypi # (23)!
pixi add --git https://github.com/mahmoud/boltons.git boltons --rev e50d4a1 --pypi # (24)!
pixi add --git https://github.com/mahmoud/boltons.git boltons --tag v0.1.0 --pypi # (25)!
pixi add --git https://github.com/mahmoud/boltons.git boltons --tag v0.1.0 --pypi --subdir boltons # (26)!
```

1. This will add the `numpy` package to the project with the latest available for the solved environment.
2. This will add multiple packages to the project solving them all together.
3. This will add the `numpy` package with the version constraint.
4. This will add the `numpy` package to the project of the manifest file at the given path.
5. This will add the `python` package as a host dependency. There is currently no different behavior for host dependencies.
6. This will add the `cmake` package as a build dependency. There is currently no different behavior for build dependencies.
7. This will add the `clang` package only for the `osx-64` platform.
8. This will add the `numpy` package to the manifest and lockfile, without installing it in an environment.
9. This will add the `numpy` package to the manifest without updating the lockfile or installing it in the environment.
10. This will add the `numpy` package in the feature `featurex`.
11. This will add the `boost-check` source package to the dependencies from the git repository.
12. This will add the `boost-check` source package to the dependencies from the git repository using `main` branch and the `boost-check` folder in the repository.
13. This will add the `boost-check` source package to the dependencies from the git repository using `v0.1.0` tag.
14. This will add the `boost-check` source package to the dependencies from the git repository using `e50d4a1` revision.
15. This will add the `requests` package as `pypi` dependency with the `security` extra.
16. This will add the `pre-release` version of `Django` to the project as a `pypi` dependency.
17. This will add the `boltons` package in the feature `lint` as `pypi` dependency.
18. This will add the `boltons` package with the given `url` as `pypi` dependency.
19. This will add the `exchangelib` package with the given `git` url as `pypi` dependency.
20. This will add the `project` package with the given `file` url as `pypi` dependency.
21. This will add the `project` package with the given `file` url as an `editable` package as `pypi` dependency.
22. This will add the `boltons` package with the given `git` url as `pypi` dependency.
23. This will add the `boltons` package with the given `git` url and `main` branch as `pypi` dependency.
24. This will add the `boltons` package with the given `git` url and `e50d4a1` revision as `pypi` dependency.
25. This will add the `boltons` package with the given `git` url and `v0.1.0` tag as `pypi` dependency.
26. This will add the `boltons` package with the given `git` url, `v0.1.0` tag and the `boltons` folder in the repository as `pypi` dependency.

!!! tip
    If you want to use a non default pinning strategy, you can set it using [pixi's configuration](../../pixi_configuration.md#pinning-strategy).
    ```
    pixi config set pinning-strategy no-pin --global
    ```
    The default is `semver` which will pin the dependencies to the latest major version or minor for `v0` versions.
!!! note
    There is an exception to this rule when you add a package we defined as non `semver`, then we'll use the `minor` strategy.
    These are the packages we defined as non `semver`:
    Python, Rust, Julia, GCC, GXX, GFortran, NodeJS, Deno, R, R-Base, Perl

--8<-- [end:example]
