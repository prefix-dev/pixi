# Hierarchical tasks

!!! warning "Preview feature"
    Hierarchical tasks are currently a **preview** feature and may change.
    Opt in by enabling the `hierarchical-tasks` preview flag in your
    workspace's `pixi.toml`:

    ```toml
    [workspace]
    # ... other workspace fields ...
    preview = ["hierarchical-tasks"]
    
    ```


    With the flag disabled, everything in this page is a no-op and your
    workspace behaves exactly as before.

## Why

When you have a monorepo of related projects — or a project that pulls
in other projects as git submodules — you often want to:

- Keep each sub-project runnable on its own (`pixi run test` inside a
  sub-project behaves normally, with no dependency on the outer
  repository).
- Aggregate the sub-projects from the outer root — list their tasks,
  depend on them, and invoke them without `cd`-ing.

The `hierarchical-tasks` preview feature adds this aggregation layer.
The design is inspired by the proposal in
[issue #5003](https://github.com/prefix-dev/pixi/issues/5003) (especially
the comments from user `phuicy`) and mirrors the way git submodules and
CMake's `add_subdirectory()` compose a larger project from smaller ones.

## How members are defined

A **member** is a subdirectory of your workspace whose manifest contains
its own `[workspace]` block (or `[tool.pixi.workspace]` for pyproject).
Each member is a **fully standalone pixi workspace**:

- Its own environments, channels, platforms.
- Its own lockfile (`<member>/pixi.lock`).
- Its own install directory (`<member>/.pixi/envs/...`).

Because a member _is_ a workspace, you can `cd` into it and run `pixi
run test`, `pixi install`, `pixi shell`, etc. — all the usual pixi
commands — without any dependence on the outer aggregation root.

Manifests with no `[workspace]` block (for example, a `pyproject.toml`
that is only a Python package, or a pixi manifest that only declares
`[package]`) are **transparent** — discovery walks right through them
as if they weren't there and keeps searching deeper.

## Discovery rules

When the preview flag is enabled, pixi walks the workspace directory
tree downward from the root manifest:

- Members form a **tree**. A member discovered inside another member
  becomes a child of the outer member. Intermediate directories that
  contain no member manifest are transparent.
- Within a given parent, member names must be **unique** (same rule as
  directory names on a filesystem). Two different parents may contain
  members with the same name — their full paths differ, so their task
  addresses differ too.
- Common build and cache directories are skipped: `.pixi`, `.git`, any
  `.`-prefixed directory, `target`, `node_modules`, `dist`, `build`,
  `venv`, `__pycache__`.

## Which workspace is "the root"? — nearest-ancestor wins

**Pixi's upward discovery rule is unchanged: the root workspace is the
nearest-ancestor `[workspace]` from your current directory.** The only
thing Model 2 changes is that a root may now _aggregate_ member
workspaces below it.

This means what you see depends on where you stand:

| CWD          | Root                | Addressable tasks                                           |
| ------------ | ------------------- | ----------------------------------------------------------- |
| `/repo`      | `/repo/pixi.toml`   | `greet`, `all_tests`, `a::test`, `a::c::test`, `b::test`    |
| `/repo/a`    | `/repo/a/pixi.toml` | `test`, `c::test` (if `a` has the preview flag on)          |
| `/repo/a/c`  | `/repo/a/c/pixi.toml` | `test`                                                    |

This is the **same mental model as git submodules**: `git status` inside
a submodule reports that submodule's state, not the outer repository's.
If you want the outer aggregation visible, run your pixi command from
the outer root.

## Running a member task

Address any member task using the `::` separator. Paths can be as deep
as the tree allows:

```console
$ pixi run a::test         # runs `test` in member `a`
$ pixi run a::c::test      # runs `test` in member `c`, a child of `a`
```

Plain task names (no `::`) resolve against the **root** workspace's
tasks — the one you're currently in. A name with `::` that doesn't
match a known top-level member also falls through to the normal task
search, so task names that happen to contain `::` keep working.

### Working directory

Because each member is its own workspace, a member task's working
directory defaults to the member's own directory. An explicit `cwd:`
declared on the task is resolved relative to the member's directory,
not the aggregation root's.

### Environment

A member task runs in the **member's own default environment** — using
the member's lockfile and its `.pixi/envs/...` install dir. This is the
key difference from plain pixi: different tasks in a single `pixi run`
invocation may execute in different workspaces, each with its own
activated environment.

## Cross-member dependencies

A task in the root may declare `depends-on` entries that reference
members using the same `::` syntax:

```toml
# <workspace root>/pixi.toml
[tasks]
all_tests = { depends-on = ["a::test", "a::c::test", "b::test"] }
```

Running `pixi run all_tests` resolves each dependency to its own member
workspace and runs it there, in member order. Cross-member cycle
detection applies across the whole graph.

## Listing tasks

`pixi task list` surfaces all reachable member tasks under their
fully-qualified names alongside the usual workspace tasks:

```console
$ pixi task list
Tasks that can run on this machine:
-----------------------------------
a::c::test, a::test, all_tests, b::test, greet
```

## Installing environments

Install timing differs between `pixi run` and `pixi install`:

- **`pixi run a::test` is lazy.** Only the member actually referenced
  by the task (and its dependencies, if any) gets its lockfile updated
  and its environment installed. Other members are untouched.

- **`pixi install` at the root is eager.** It walks every reachable
  member workspace and installs each one's default environment, writing
  a `pixi.lock` in each. This is useful for a cold checkout where you
  want everything ready in one go.

## Example

```text
my-workspace/
├── pixi.toml                # [workspace] preview = ["hierarchical-tasks"]
│                            # [tasks] all_tests = { depends-on = [...] }
├── a/
│   ├── pixi.toml            # [workspace] name = "a" + [tasks]
│   └── c/
│       └── pixi.toml        # [workspace] name = "c" + [tasks]
└── b/
    └── pixi.toml            # [workspace] name = "b" + [tasks]
```

```console
$ pixi run a::test            # from my-workspace: runs in a/
$ pixi run a::c::test         # from my-workspace: runs in a/c/
$ pixi run all_tests          # from my-workspace: runs all three, each in its own workspace

$ cd a/c
$ pixi run test               # standalone: upward walk stops at a/c/pixi.toml;
                              # outer aggregation is invisible from inside
```

## Scope and limitations

This preview covers the **task layer**:

- **Yes**: downward member discovery, `a::b::task` addressing,
  cross-member `depends-on`, per-member working directory, per-member
  lockfiles and install dirs, `pixi task list` showing the full tree,
  `pixi install` eagerly installing every member.
- **Not yet**: merging dependencies across members into a single solve,
  a unified lockfile spanning members, public/private dependency scope
  between members, `--package`/`-w` flag for addressing, `pixi task
add a::new_task` (edit the member's `pixi.toml` directly).

These are tracked as follow-ups on
[issue #5003](https://github.com/prefix-dev/pixi/issues/5003).
