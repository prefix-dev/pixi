# `conda-script` examples

Stress-test corpus for the [`conda-script` proposal](https://github.com/prefix-dev/pixi/issues/3751).

The proposal embeds conda metadata (channels, dependencies, entrypoint) in a
comment block inside a single code file, so a script can be shared and run
without a separate project file. It is not implemented in pixi yet; these
examples exist to pressure-test the draft spec against the top 30 programming
languages before it becomes a CEP.

Every folder contains exactly one file: the script is the whole project. Each
script declares its toolchain plus one library it genuinely uses, prints
deterministic output, and was verified by hand on linux-64 by materializing
the declared environment with `pixi exec` and running the entrypoint commands.
`./run-all.sh` runs every example in one go against the locally built pixi;
pass another binary as the first argument to verify that one instead.

Languages are ordered by TIOBE rank, filtered to those where a single-file
script is meaningful and the toolchain is installable from conda channels.
Folder numbers follow that order; the numbering is not contiguous because
some languages were attempted, verified, and then removed as too awkward to
recommend (see the second exclusion table).

The corpus adopts three adjustments to the draft in issue 3751:

- The entrypoint runs in the directory the user invoked the tool from, not in
  a tool-owned directory. Relative-path arguments to the script therefore work.
- A `${CACHE}` variable points at a persistent per-script directory owned by
  the tool, for build artifacts and fetcher state, so nothing lands in the
  user's working directory.
- Command substitution `$(...)` is allowed. A failing inner command aborts the
  entrypoint, output is stripped of the trailing newline and whitespace-split
  when unquoted. pixi's `deno_task_shell` dependency already parses and
  executes it. In exchange, `${PREFIX}` is dropped: no example needs it.

## The examples

| # | Language | Toolchain | Library | How the library arrives |
| --- | --- | --- | --- | --- |
| 01 | Python | `python` | `pyyaml` | conda environment |
| 02 | C++ | `gxx` | `fmt` | conda environment, compiler default paths |
| 03 | C | `gcc` + `pkg-config` | `glib` | conda environment, flags via `$(pkg-config ...)` |
| 05 | C# | `dotnet` | `Newtonsoft.Json` | NuGet via `#:package` directive |
| 07 | Go | `go` + `gcc`, `pkg-config` | `zlib` | conda environment, cgo via `#cgo pkg-config` |
| 08 | Fortran | `gfortran` | `liblapack` | conda environment, compiler default paths |
| 10 | Perl | `perl` | `perl-uri` | conda environment |
| 11 | R | `r-base` | `r-jsonlite` | conda environment |
| 12 | Rust | `rust` + `gcc` | `zlib` | conda environment, FFI via `#[link]` |
| 13 | Kotlin | `kotlin` | `gson` | Maven via `@file:DependsOn` |
| 14 | Ruby | `ruby` | `rb-addressable` | conda environment |
| 15 | Julia | `julia` | `JSON` | Pkg, into the environment's depot |
| 17 | TypeScript | `deno` | `lodash-es` | deno's `npm:` imports |
| 18 | Scala | `scala3` | `upickle` | coursier via `//> using dep` |
| 19 | Lua | `lua` | `lua-luafilesystem` | conda environment |
| 21 | Bash | `brush` | `jq` | conda environment (dependency program) |
| 22 | PowerShell | `powershell` | `powershell-yaml` | PSGallery via `Install-Module` |
| 23 | Zig | `zig` + `pkg-config` | `zlib` | conda environment, flags via `$(pkg-config ...)` |
| 24 | Mojo | `mojo` (Modular channel) | `numpy` | conda environment, Python interop |

All 19 examples run as written on linux-64 with byte-identical output across
repeated runs, and none writes a file into the invocation directory.

## Variable usage tally

- `${SCRIPT}`: all 19.
- `${CACHE}`: 5 (C, C++, Fortran, Rust for build artifacts; Scala for its
  build workspace).
- `${PREFIX}`: 0, and the corpus proposes dropping the variable. Five
  examples once used it for include, lib, and rpath flags; the conda-forge
  compilers turned out to search `$PREFIX/include` and `$PREFIX/lib` by
  default and bake an rpath to `$PREFIX/lib` into every binary they link,
  and `$(pkg-config ...)` covers the rest (Zig's bundled linker, glib's
  subdirectory headers).
- An `[env]` table was considered and is NOT needed: the two candidates that
  required one (COBOL, JavaScript via node) were removed with their languages.

## Findings

### Entrypoints come in two shapes

1. One command that runs the file: 15 of 19, from `python ${SCRIPT}` to
   `zig run ${SCRIPT} $(pkg-config --libs zlib)`. Languages whose dependencies
   come from a native fetcher keep this shape, because the fetching is
   declared in-file, not in the entrypoint.
2. Compile into `${CACHE}`, then run:
   `g++ -o ${CACHE}/main ${SCRIPT} -lfmt && ${CACHE}/main` (C, C++, Fortran,
   Rust).

### In-file dependency directives are widespread prior art

Kotlin's `@file:DependsOn`, .NET 10's `#:package`, Scala's `//> using dep`,
deno's `npm:` import specifiers, and Julia's in-script `Pkg` calls all embed
dependency metadata in a single runnable file. The conda-script block
composes cleanly with every one of them, and adds the thing none of them can
express: pinning the runtime itself. A file can carry both its conda block
and its native directives without conflict.

### The mini-shell holds

Every kept example fits the grammar: whitespace splitting, quotes, `${VAR}`,
`&&`, and `$(...)`. Command substitution earns its place through one library
class: packages whose headers live outside `$PREFIX/include` (glib's
`include/glib-2.0` plus a generated header under `lib/glib-2.0/include`) are
inexpressible without `$(pkg-config --cflags --libs ...)`, and it also
replaced the last uses of `${PREFIX}`. The known remaining limitation is that
"run this only if absent" is inexpressible (no `||`, no conditionals), so
idempotency must come from the invoked tools. Go is where this bit:
`go mod init` fails on rerun, which is why the Go example uses cgo instead of
Go modules. All kept examples also keep stdout clean; fetcher and compiler
chatter goes to stderr.

### The environment does the heavy lifting

The conda-forge compilers resolve everything relative to their own install
location: `$PREFIX/include` and `$PREFIX/lib` are on their default search
paths and their specs bake an rpath to `$PREFIX/lib` into every linked
binary. `g++ -o ${CACHE}/main ${SCRIPT} -lfmt` is a complete C++ entrypoint
with no path flags at all; only subdirectory-header libraries like glib need
`$(pkg-config --cflags --libs ...)` on top, and conda's patched `pkg-config`
resolves the prefix itself (cgo's in-file `#cgo pkg-config: zlib` works the
same way without the substitution).
Activation matters the same way: cgo picks up `CC` and `CGO_ENABLED` from
the go package's activation, and Julia's `JULIA_DEPOT_PATH` points into the
prefix. The CEP should guarantee activation as a contract, including
exported activation variables.

### Feedstock and packaging issues found along the way

- conda-forge `julia` fails to start on glibc >= 2.41 hosts because the
  `openlibm` feedstock ships an executable-stack shared object (verification
  needed `GLIBC_TUNABLES=glibc.rtld.execstack=2` as a host-side workaround).
- conda-forge `scala3` identifies itself as `3.7.4-bin-SNAPSHOT`, a version
  coursier cannot fetch; the script must pin `//> using scala 3.7.4`.
- Solver skew: `rb-*` gems pin old ruby minors, `lua-luafilesystem` pins lua
  5.4, `perl-uri` pins perl 5.32.

### Caches outside the tool's control

NuGet, deno, PSGallery modules, coursier, Kotlin's Maven resolver, zig, and
go all cache under `$HOME`, shared across environments and never cleaned up
with them. Redirection knobs exist (`DENO_DIR`, `COURSIER_CACHE`, ...) but
setting them would need env-var support in the spec. A hermetic runner is not achievable today; the CEP should decide
whether that is acceptable.

### Smaller spec notes

- Kotlin's dependency-resolving scripts must be named `*.main.kts`, so a
  fixed `main.<ext>` naming convention is impossible; the spec should leave
  filenames free-form.
- The Bash example uses `brush`, a bash-compatible shell from conda-forge; a
  host bash exists on every Linux machine, so only a non-host interpreter
  proves the environment actually supplied it. For the same reason the Rust
  example declares `gcc` and passes `-C linker=gcc` instead of relying on
  rustc's default host `cc`; the conda gcc then also finds `-lz` and bakes
  the rpath on its own.
- The Python example deliberately uses a conda-script block instead of
  PEP 723 to show the block works there too; the precedence rule in the
  proposal is being reworded to allow either.

## Excluded languages

Never attempted:

| Language | Reason |
| --- | --- |
| Visual Basic | the compiler ships in the conda-forge `dotnet` SDK, but .NET file-based apps support only C#; classic VB is proprietary |
| SQL | not a standalone script; needs a database to be meaningful |
| MATLAB | proprietary; Octave, the open implementation of the language family, was attempted and removed (see the second table) |
| Assembly | no meaningful single-library story for a portability-focused spec |
| Scratch | not a text-based language |
| Delphi / Object Pascal | no Free Pascal or Delphi toolchain on conda-forge |
| SAS | proprietary |
| Swift | no Swift toolchain on conda-forge (only unrelated `swift-sim`) |

Attempted, verified, and removed as too awkward:

| Language | What worked | Why it was removed |
| --- | --- | --- |
| Java | `java --class-path ${PREFIX}/lib/antlr4.jar ${SCRIPT}` | `antlr` is the only usable jar on conda-forge; `jbang` (the natural single-file runner) is not packaged either |
| JavaScript | `node ${SCRIPT}` with an absolute `require` built from `CONDA_PREFIX` | node cannot see `$PREFIX/lib/node_modules` without `NODE_PATH`, and `npx --package` stopped exporting `NODE_PATH` in npm 7, so only executables resolve, not libraries; deno (`17-typescript`) covers the ecosystem |
| PHP | a three-segment entrypoint bootstrapping composer into `${CACHE}` | composer is not on conda-forge, and the bootstrap re-downloads `composer.phar` on every run because the mini-shell has no conditionals |
| Haskell | FFI to `zlib` compiled with `ghc` | conda-forge `ghc` is frozen at 8.10.7 from 2021, `cabal-install` is not packaged, and ghc prints compile chatter to stdout |
| Dart | `dart create` scaffolding a throwaway project inside `${CACHE}` | dart has no single-file dependency mechanism at all; the scaffold drags in 48 dev dependencies and prints to stdout on every run |
| COBOL | `cobc` linking conda-forge `zlib` via static `CALL` | the `gnucobol` feedstock hardcodes `COB_CC` to its build path, so compiling requires env vars the entrypoint cannot set |
| Groovy | `groovy ${SCRIPT}` with `commons-lang3` via `@Grab` | Grape resolves into `~/.groovy/grapes` with no way to redirect it from the spec; Kotlin (13) already covers the JVM-with-Maven-directives story |
| Octave | `octave-statistics` via `pkg load` | the entrypoint must lead with a `pkg rebuild -global` repair segment on every run because pixi does not run the feedstock's post-link scripts, and that segment mutates the environment globally |
| Prolog | `list_util` via an in-script conditional `pack_install` | "install only if absent" needs a three-directive preamble in the script because the mini-shell has no conditionals, and SWI packs land in a per-user directory outside the environment |
| Common Lisp | FFI to conda-forge `zlib` via `sb-alien` | no Lisp package manager is on conda-forge, so hand-written FFI against `CONDA_PREFIX` is the only library story |
| Elixir | `jason` via `Mix.install` | hex and the `Mix.install` build cache live under `$HOME` with no way to redirect them from the spec |
