# Hardened Build Backends

A **hardened backend** is a build backend that can be run on a repository that nobody has reviewed.
It follows the [requirements](#requirements) on this page while it reports package metadata.

This page is for authors of build backends, and for operators who decide which backends to [trust](../advanced/untrusted_repositories.md#trusting-build-backends).
The two words mean different things here.
Hardened is a property of the backend, which its authors are responsible for.
Trusting a backend is a decision of the operator, who lists it in `security.trusted-packages`.
Operators should only trust hardened backends.

A backend does not have to be hardened.
One that is not should only not be trusted while untrusted code execution is disabled.

## Why backends are special

When [untrusted code execution is disabled](../advanced/untrusted_repositories.md), Pixi refuses to run anything the repository brings along, with one exception.
A workspace with source dependencies cannot be solved without asking a build backend for the package metadata, so Pixi still runs the backends the operator has listed as trusted.

A trusted backend is therefore the one program that runs on the operator's machine while it reads files written by an attacker.
If the backend can be talked into running the repository's code, the protection is gone.
Hardening is what prevents that.

## When a backend runs

Pixi talks to a backend in two phases:

| Phase    | Requests                                             | When                                                    |
|----------|------------------------------------------------------|---------------------------------------------------------|
| Metadata | `negotiateCapabilities`, `initialize`, `conda/outputs` | While solving, for example in `pixi lock`.              |
| Build    | `conda/build_v1`                                     | When the package is built, for example in `pixi install`. |

With untrusted code execution disabled, Pixi only reaches the metadata phase.
It never sends `conda/build_v1`.

The requirements below apply to the metadata phase, including everything the backend does when its process starts.
The build phase runs the repository's build by design and has no such requirements.

Pixi does not tell the backend whether untrusted code execution is disabled.
A hardened backend meets the requirements in every metadata phase.

## What the repository controls

A hardened backend treats all of the following as written by an attacker:

- every file in the source directory, including the manifest, `recipe.yaml`, `pyproject.toml`, `Cargo.toml`, `package.xml` and symbolic links;
- the project model that Pixi passes in `initialize`: name, version, description, license, dependencies and so on;
- the backend configuration from `[package.build.config]` and its target-specific variants;
- variant values and variant files;
- the channels of the workspace;
- for git and URL sources, the location the source was fetched from.

The repository does not control the environment variables of the backend process, the cache and work directories that Pixi passes, or the packages in the backend's own environment.

## Requirements

A hardened backend follows these rules during the metadata phase.

1. **Run nothing from the repository.**
   Do not execute scripts, do not import the project's code, and do not call a tool that evaluates project files as code.
   Examples of what this excludes: running `setup.py`, configuring a CMake project, running a Cargo build script.

2. **Start no process with arguments from the repository.**
   A string from the repository that reaches a command line can turn into an option of that command.
   If a backend must start a process, the program and its options are fixed, and repository strings are validated and passed after a `--` separator.

3. **Read only inside the source directory.**
   A path from the repository can be absolute, can contain `..`, or can go through a symbolic link.
   Resolve the path and check that it is still inside the source directory before reading it.
   This also holds for files a format refers to, such as a parent workspace manifest.

4. **Keep the environment private.**
   The backend process may hold tokens in its environment variables.
   They must not appear in the returned metadata, in error messages or in logs.
   Do not give repository input a way to read environment variables.

5. **Contact no host the repository chooses.**
   Network requests go to fixed hosts only, and the repository must not control more of the request than a validated name.

6. **Treat repository strings as data.**
   If the backend uses a template engine, a string from the repository must never be evaluated as a template.
   Escape such strings before they are written into something that is rendered later, for example a generated recipe.

7. **Write only where Pixi allows it.**
   That is the work directory of the request and the cache directories from `initialize`.

8. **No configuration option may relax these rules.**
   The configuration comes from the repository.
   An option that enables code execution, file access or network access must have no effect in the metadata phase.

Error messages count as output.
On a bot they end up in logs and pull request comments that the attacker can read.

The packages in the backend's environment are trusted together with the backend, so keep that environment small.
Operators have to list every one of them.

## When a backend needs to run code

Some projects only reveal their metadata when code runs.
A Python project with a `setup.py` is an example: its version and dependencies can be computed by the project's own code.
A recipe that is a template is another.
A backend that supports such projects has to evaluate what the repository wrote, and that is a legitimate thing for a backend to do.

It cannot be combined with the requirements above.
Pixi does not tell the backend whether untrusted code execution is disabled, so a backend cannot run code in one case and refuse in the other.
A backend therefore has two options:

- It never runs code from the repository to get metadata, and fails with a clear error for a project that would need it.
  Then it can be hardened.
- It runs code from the repository when a project needs it.
  Then it is not hardened, and its documentation should say so.

## Backends in this repository

The metadata phase of the following backends has been reviewed against the requirements above.

| Backend                    | Status                                 |
|----------------------------|----------------------------------------|
| `pixi-build-cmake`         | Hardened from version: to be added     |
| `pixi-build-mojo`          | Hardened from version: to be added     |
| `pixi-build-python`        | Hardened from version: to be added     |
| `pixi-build-r`             | Hardened from version: to be added     |
| `pixi-build-ros`           | Hardened from version: to be added     |
| `pixi-build-rust`          | Hardened from version: to be added     |
| `pixi-build-rattler-build` | Not hardened, by design                |

A backend or a version that is not listed has not been reviewed.
Use the listed version as the lower bound of `version` in your [trusted packages](../advanced/untrusted_repositories.md#trusting-build-backends).

`pixi-build-rattler-build` is a backend that needs to run code.
Its input is a `recipe.yaml`, which is a template that the repository writes, and evaluating that template is how the backend produces metadata.
Do not trust it on untrusted repositories.
A source dependency that uses it cannot be solved while untrusted code execution is disabled.

Hardened is not a guarantee.
It means that the maintainers read the code of the metadata phase and found nothing that breaks the requirements.
