//! Extracts the exact set of input files for a CMake/Ninja build by querying
//! ninja's view of the build graph after the build has run.
//!
//! Three ninja sub-commands cover most of what we need:
//! * `ninja -t inputs all`: the declared translation units (the `.cc`/`.cpp`
//!   files that targets list as their sources). These do *not* show up in
//!   `-t deps`.
//! * `ninja -t deps`: the transitive header chain, loaded from the depfile
//!   DB (`.ninja_deps`) that the compiler emits during the build.
//! * `ninja -t targets all`: surfaces `CMakeLists.txt` and the `*.cmake`
//!   files that CMake registers as inputs to its regen rule (they appear as
//!   `<path>: phony` entries).
//!
//! On top of that we read `<build>/CMakeFiles/VerifyGlobs.cmake` if it
//! exists. CMake writes that file whenever `file(GLOB ... CONFIGURE_DEPENDS
//! ...)` is used, and it records the original glob patterns. We forward
//! those patterns so that adding a new file matching one of them properly
//! invalidates pixi's build cache. Plain `file(GLOB)` (without
//! `CONFIGURE_DEPENDS`) writes nothing, by design: that's CMake's own
//! "you opted out of auto-detection" footgun and we follow the same
//! semantics.
//!
//! Paths outside the source dir are dropped (system headers, CMake-shipped
//! Modules, files in unrelated source trees). Paths under `<source_dir>/.pixi/`
//! are also dropped: that's where pixi materializes its conda envs and caches,
//! which are tracked by pixi's environment hash, not by the input set.

use std::{
    collections::BTreeSet,
    io,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const NINJA_BUILD_DIR: &str = "build";
const PIXI_CACHE_DIR: &str = ".pixi";
const CMAKE_HOME_DIRECTORY_KEY: &str = "CMAKE_HOME_DIRECTORY:INTERNAL=";
const CMAKE_MAKE_PROGRAM_KEY: &str = "CMAKE_MAKE_PROGRAM:FILEPATH=";
const VERIFY_GLOBS_FILE: &str = "CMakeFiles/VerifyGlobs.cmake";

/// Returns the exact set of source-relative input paths that drove this CMake
/// build, or an error if any step (cache read, ninja invocation, parsing)
/// fails. Callers should treat any error as "fall back to globs".
pub fn exact_inputs_from_ninja(workdir: &Path) -> io::Result<BTreeSet<String>> {
    let build_dir = cmake_build_dir(workdir);
    if !build_dir.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("cmake build dir not found at {}", build_dir.display()),
        ));
    }

    let cache = fs_err::read_to_string(build_dir.join("CMakeCache.txt"))?;
    let source_dir = read_cache_value(&cache, CMAKE_HOME_DIRECTORY_KEY)
        .map(PathBuf::from)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "CMAKE_HOME_DIRECTORY not found in CMakeCache.txt",
            )
        })?;
    // The build env's ninja typically isn't on PATH for the backend process;
    // CMake records the binary it picked, so we use that.
    let ninja_binary = read_cache_value(&cache, CMAKE_MAKE_PROGRAM_KEY)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("ninja"));
    let ninja = Ninja::new(ninja_binary, build_dir.clone());

    let outputs = ninja.run_concurrently([
        ("-t inputs all", &["-t", "inputs", "all"]),
        ("-t deps", &["-t", "deps"]),
        ("-t targets all", &["-t", "targets", "all"]),
    ])?;
    for (label, output) in outputs.iter() {
        if !output.status.success() {
            return Err(io::Error::other(format!(
                "`ninja {label}` exited with {}",
                output.status
            )));
        }
    }
    let [(_, inputs), (_, deps), (_, targets)] = outputs;

    let layout = Layout::new(&source_dir, &build_dir);
    let mut files = BTreeSet::new();
    parse_inputs(decode_stdout(&inputs.stdout)?, &layout, &mut files);
    parse_deps(decode_stdout(&deps.stdout)?, &layout, &mut files);
    parse_targets(decode_stdout(&targets.stdout)?, &layout, &mut files);

    // Recover the original CONFIGURE_DEPENDS glob patterns so that adding a
    // new file matching one of them invalidates the cache. Best-effort: a
    // missing or malformed VerifyGlobs.cmake is not fatal.
    let verify_path = build_dir.join(VERIFY_GLOBS_FILE);
    if let Ok(verify_globs) = fs_err::read_to_string(&verify_path) {
        parse_verify_globs(&verify_globs, &layout, &mut files);
    }

    Ok(files)
}

/// rattler-build runs the build script with cwd = `<work_directory>/work`,
/// and the cmake script does `pushd build` from there.
fn cmake_build_dir(workdir: &Path) -> PathBuf {
    workdir.join("work").join(NINJA_BUILD_DIR)
}

/// Returns the value of a `KEY:TYPE=value` entry from a parsed
/// `CMakeCache.txt`, or `None` if the key isn't present.
fn read_cache_value<'a>(cache: &'a str, key: &str) -> Option<&'a str> {
    cache.lines().find_map(|l| l.strip_prefix(key))
}

/// Decode ninja stdout as UTF-8. Ninja writes UTF-8 in practice; if it ever
/// produces something else we'd rather surface the error and let the caller
/// fall back to globs than silently drop lines.
fn decode_stdout(bytes: &[u8]) -> io::Result<&str> {
    std::str::from_utf8(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// The two normalized roots that turn build-graph paths into source-relative
/// inputs, plus the rule that drops paths outside the source dir or under
/// `.pixi/`. Constructing one normalizes both paths to forward slashes with
/// no trailing slash so all downstream comparisons are textual.
struct Layout {
    source_dir: String,
    build_dir: String,
}

impl Layout {
    fn new(source_dir: &Path, build_dir: &Path) -> Self {
        Self {
            source_dir: normalized_path(source_dir),
            build_dir: normalized_path(build_dir),
        }
    }

    /// Returns `path` as a forward-slash, source-relative string if it sits
    /// inside the source dir and outside the pixi cache (`.pixi/`).
    ///
    /// `path` may be absolute or relative to the build dir; relative paths are
    /// canonicalized lexically against the build dir before the source-dir
    /// prefix check. Backslashes are normalized to forward slashes throughout
    /// (CMake/ninja emit either form on Windows depending on the sub-tool).
    fn relativize(&self, path: &str) -> Option<String> {
        let absolute = canonicalize(path, &self.build_dir);
        self.strip_source_root(&absolute)
    }

    /// Converts an absolute CMake glob pattern into a source-relative,
    /// forward-slash, gitignore-style pattern. Returns `None` if the pattern
    /// is outside the source dir or under `.pixi/`. `recursive` matches
    /// `file(GLOB_RECURSE)` semantics: searches every subdir of the dir
    /// component, so `<dir>/<pat>` becomes `<dir>/**/<pat>`.
    fn relativize_glob(&self, pattern: &str, recursive: bool) -> Option<String> {
        let normalized = pattern.replace('\\', "/");
        let rel = self.strip_source_root(&normalized)?;
        if !recursive || rel.contains("**") {
            return Some(rel);
        }
        // `file(GLOB_RECURSE … "<dir>/<pat>")` searches every subdir of <dir>.
        // The matching gitignore-style form is `<dir>/**/<pat>`.
        match rel.rsplit_once('/') {
            Some((dir, last)) => Some(format!("{dir}/**/{last}")),
            None => Some(format!("**/{rel}")),
        }
    }

    /// Inner helper: strip the source-dir prefix and reject empty results
    /// or paths that fall under `.pixi/`. Both `relativize` paths funnel
    /// through here so the "what counts as a source input" rule lives in
    /// exactly one place.
    fn strip_source_root(&self, absolute: &str) -> Option<String> {
        let rest = absolute.strip_prefix(&self.source_dir)?;
        let rel = rest.strip_prefix('/').unwrap_or(rest);
        if rel.is_empty() || is_pixi_cache_segment(rel) {
            return None;
        }
        Some(rel.to_string())
    }
}

/// Wraps the ninja binary plus the build dir so callers don't repeat the
/// `Command::new(...).arg("-C").arg(build_dir).args(...)` chain.
struct Ninja {
    binary: PathBuf,
    build_dir: PathBuf,
}

impl Ninja {
    fn new(binary: PathBuf, build_dir: PathBuf) -> Self {
        Self { binary, build_dir }
    }

    /// Runs each `(label, args)` query in its own thread so none can block
    /// on a full pipe buffer while the others wait. The label is propagated
    /// alongside its `Output` so callers can attribute failures.
    fn run_concurrently(
        &self,
        queries: [(&'static str, &[&str]); 3],
    ) -> io::Result<[(&'static str, Output); 3]> {
        std::thread::scope(|s| {
            let handles = queries.map(|(label, args)| {
                let h = s.spawn(move || {
                    Command::new(&self.binary)
                        .arg("-C")
                        .arg(&self.build_dir)
                        .args(args)
                        .output()
                });
                (label, h)
            });
            let collected: Vec<(&'static str, Output)> = handles
                .into_iter()
                .map(|(label, h)| h.join().expect("ninja thread panicked").map(|o| (label, o)))
                .collect::<io::Result<_>>()?;
            Ok(collected.try_into().expect("3 ninja queries -> 3 outputs"))
        })
    }
}

/// `ninja -t inputs all` lists every input edge of the named targets,
/// one per line: sources, link inputs, even intermediate object files.
/// Anything inside `source_dir` that isn't under `.pixi/` is a user input.
fn parse_inputs(stdout: &str, layout: &Layout, files: &mut BTreeSet<String>) {
    for line in nonempty_lines(stdout) {
        if let Some(rel) = layout.relativize(line) {
            files.insert(rel);
        }
    }
}

/// `ninja -t deps` emits stanzas like:
///
/// ```text
/// foo.o: #deps 5, deps mtime 1234 (VALID)
///     foo.cc
///     ../include/foo.h
/// bar.o: #deps 3, deps mtime 1234 (VALID)
///     /abs/bar.cc
/// ```
///
/// Indented lines are headers (and sometimes the TU) discovered by the
/// compiler. They may be either absolute or relative to the build dir.
fn parse_deps(stdout: &str, layout: &Layout, files: &mut BTreeSet<String>) {
    for line in nonempty_lines(stdout) {
        // Dep entries are indented by exactly four spaces.
        let Some(path) = line.strip_prefix("    ") else {
            continue;
        };
        if let Some(rel) = layout.relativize(path) {
            files.insert(rel);
        }
    }
}

/// `ninja -t targets all` emits `<path>: <rule>` lines. CMake's regen-rule
/// inputs (CMakeLists.txt, included `.cmake` files) appear as `<path>: phony`
/// with **absolute** paths: CMake records them with full paths in the
/// regen edge.
///
/// `-t targets all` also emits synthetic phony aliases for build targets
/// (`all`, `clean`, `<target_name>`) using bare relative names. Those would
/// otherwise canonicalize into the build dir (which may sit under the
/// source dir) and pollute the result, so we reject any non-absolute path.
fn parse_targets(stdout: &str, layout: &Layout, files: &mut BTreeSet<String>) {
    for line in nonempty_lines(stdout) {
        let Some((path, rule)) = line.rsplit_once(':') else {
            continue;
        };
        if rule.trim() != "phony" {
            continue;
        }
        let path = path.trim();
        if !is_absolute(path) {
            continue;
        }
        if let Some(rel) = layout.relativize(path) {
            files.insert(rel);
        }
    }
}

/// `VerifyGlobs.cmake` is the file CMake writes when `CONFIGURE_DEPENDS` is
/// in play. Each tracked glob shows up as a single `file(GLOB ...)` or
/// `file(GLOB_RECURSE ...)` line, with one absolute pattern per line:
///
/// ```text
/// file(GLOB NEW_GLOB LIST_DIRECTORIES true "/abs/src/*.cc")
/// file(GLOB_RECURSE NEW_GLOB LIST_DIRECTORIES false "/abs/include/*.hpp")
/// ```
///
/// We translate each pattern into a source-relative gitignore-style glob
/// and add it to the input set. `GLOB_RECURSE` becomes `dir/**/<pattern>`
/// to match pixi's matcher semantics (the `ignore` crate).
fn parse_verify_globs(text: &str, layout: &Layout, files: &mut BTreeSet<String>) {
    for line in text.lines() {
        let trimmed = line.trim_start();
        let (recursive, rest) = if let Some(r) = trimmed.strip_prefix("file(GLOB_RECURSE ") {
            (true, r)
        } else if let Some(r) = trimmed.strip_prefix("file(GLOB ") {
            (false, r)
        } else {
            continue;
        };
        let Some(rest) = rest.strip_suffix(')') else {
            continue;
        };
        let Some(pattern) = first_quoted(rest) else {
            continue;
        };
        if let Some(rel) = layout.relativize_glob(pattern, recursive) {
            files.insert(rel);
        }
    }
}

/// Iterates non-empty lines from a UTF-8 stdout buffer. `str::lines()` already
/// handles both `\n` and `\r\n` terminators and never yields the terminator
/// itself, so the parsers don't need to think about line endings.
fn nonempty_lines(stdout: &str) -> impl Iterator<Item = &str> {
    stdout.lines().filter(|l| !l.is_empty())
}

/// Extracts the first `"…"` substring from a slice. Returns `None` if no
/// quoted token is present.
fn first_quoted(s: &str) -> Option<&str> {
    let start = s.find('"')? + 1;
    let rel = &s[start..];
    let end = rel.find('"')?;
    Some(&rel[..end])
}

/// Canonicalize a build-graph path string into an absolute, forward-slash
/// path. Resolves `..` and `.` segments lexically (no filesystem access:
/// the build dir's symlink topology is irrelevant for our matching).
fn canonicalize(path: &str, build_dir: &str) -> String {
    let normalized = path.replace('\\', "/");
    let absolute = if is_absolute(&normalized) {
        normalized
    } else {
        format!("{build_dir}/{normalized}")
    };

    // Lexical resolution of `.` and `..` segments. We can't use
    // `Path::canonicalize` because we want pure string handling and we
    // don't need to touch the filesystem.
    let mut out: Vec<&str> = Vec::new();
    let mut leading_root = String::new();
    let after_root = if let Some(rest) = absolute.strip_prefix('/') {
        leading_root.push('/');
        rest
    } else if absolute.len() >= 3
        && absolute.as_bytes()[1] == b':'
        && absolute.as_bytes()[2] == b'/'
    {
        // Windows drive root e.g. "F:/..."
        leading_root.push_str(&absolute[..3]);
        &absolute[3..]
    } else {
        absolute.as_str()
    };

    for seg in after_root.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    format!("{leading_root}{}", out.join("/"))
}

fn is_absolute(p: &str) -> bool {
    p.starts_with('/')
        || (p.len() >= 3
            && p.as_bytes()[1] == b':'
            && (p.as_bytes()[2] == b'/' || p.as_bytes()[2] == b'\\'))
}

/// True if the first path segment is `.pixi`: i.e. the file sits inside
/// pixi's cache/env directory at the source root.
fn is_pixi_cache_segment(rel: &str) -> bool {
    rel == PIXI_CACHE_DIR || rel.starts_with(&format!("{PIXI_CACHE_DIR}/"))
}

/// Forward-slash, no trailing slash. CMake/ninja emit forward slashes for
/// some sub-tools and backslashes for others, so we normalize to forward
/// slashes everywhere.
fn normalized_path(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    s.trim_end_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(source_dir: &str, build_dir: &str) -> Layout {
        Layout::new(Path::new(source_dir), Path::new(build_dir))
    }

    fn collect_inputs(stdout: &str, source_dir: &str, build_dir: &str) -> Vec<String> {
        let mut set = BTreeSet::new();
        parse_inputs(stdout, &layout(source_dir, build_dir), &mut set);
        set.into_iter().collect()
    }

    fn collect_deps(stdout: &str, source_dir: &str, build_dir: &str) -> Vec<String> {
        let mut set = BTreeSet::new();
        parse_deps(stdout, &layout(source_dir, build_dir), &mut set);
        set.into_iter().collect()
    }

    fn collect_targets(stdout: &str, source_dir: &str, build_dir: &str) -> Vec<String> {
        let mut set = BTreeSet::new();
        parse_targets(stdout, &layout(source_dir, build_dir), &mut set);
        set.into_iter().collect()
    }

    fn collect_verify_globs(text: &str, source_dir: &str) -> Vec<String> {
        let mut set = BTreeSet::new();
        parse_verify_globs(text, &layout(source_dir, ""), &mut set);
        set.into_iter().collect()
    }

    #[test]
    fn inputs_keeps_source_files_drops_outputs_and_externals() {
        // Mirrors actual `ninja -t inputs all` output on Windows: backslashes
        // for build-relative paths, absolute paths for things found via
        // CMake's host/build prefixes.
        let stdout = "\
.
CMakeFiles\\sdl_example.dir\\src\\main.cc.obj
F:\\projects\\proj\\.pixi\\build\\work\\foo\\host\\Library\\lib\\SDL2.lib
F:\\projects\\proj\\src\\main.cc
bin\\sdl_example.exe
";
        assert_eq!(
            collect_inputs(
                stdout,
                "F:/projects/proj",
                "F:/projects/proj/.pixi/build/work/foo/work/build"
            ),
            vec!["src/main.cc"]
        );
    }

    #[test]
    fn deps_keeps_project_paths_drops_externals_and_pixi_cache() {
        let stdout = "\
CMakeFiles/probe.dir/src/main.cc.obj: #deps 5, deps mtime 1234 (VALID)
    /src/proj/src/main.cc
    /src/proj/include/greet.hpp
    /src/proj/.pixi/envs/default/include/iostream
    /opt/conda/include/c++/iostream
    /usr/include/stddef.h
CMakeFiles/probe.dir/src/util.cc.obj: #deps 2, deps mtime 1234 (VALID)
    /src/proj/src/util.cc
    /src/proj/include/util.hpp
";
        assert_eq!(
            collect_deps(stdout, "/src/proj", "/src/proj/build"),
            vec![
                "include/greet.hpp",
                "include/util.hpp",
                "src/main.cc",
                "src/util.cc",
            ]
        );
    }

    #[test]
    fn deps_canonicalizes_relative_paths_against_build_dir() {
        // Real-world cpp-sdl shape: build dir is
        // `<src>/.pixi/build/work/foo/work/build`, headers from a sibling
        // `host/` show up as `../../host/...` in `-t deps` output.
        let stdout = "\
CMakeFiles/sdl_example.dir/src/main.cc.obj: #deps 2, deps mtime 1234 (VALID)
    ../../../../../../src/main.cc
    ../../host/Library/include/SDL2/SDL.h
";
        assert_eq!(
            collect_deps(
                stdout,
                "F:/projects/proj",
                "F:/projects/proj/.pixi/build/work/foo/work/build"
            ),
            vec!["src/main.cc"]
        );
    }

    #[test]
    fn targets_keeps_project_phony_drops_cmake_modules_outputs_and_pixi_cache() {
        let stdout = "\
all: phony
probe: phony
probe.exe: CXX_EXECUTABLE_LINKER__probe_
CMakeFiles/probe.dir/src/main.cc.obj: CXX_COMPILER__probe_unscanned_
build.ninja: RERUN_CMAKE
/src/proj/CMakeLists.txt: phony
/src/proj/cmake/Helpers.cmake: phony
/src/proj/.pixi/envs/default/share/cmake-4.3/Modules/Foo.cmake: phony
/opt/cmake/share/cmake-4.3/Modules/CMakeCXXInformation.cmake: phony
clean: CLEAN
";
        assert_eq!(
            collect_targets(stdout, "/src/proj", "/src/proj/build"),
            vec!["CMakeLists.txt", "cmake/Helpers.cmake"]
        );
    }

    #[test]
    fn relativize_handles_windows_backslashes_and_mixed_slashes() {
        // CMake emits forward slashes even on Windows; the source dir we
        // read from CMakeCache.txt may not have a trailing slash.
        let layout = layout("C:/work/probe", "C:/work/probe/build");
        assert_eq!(
            layout.relativize("C:/work/probe/src/main.cc"),
            Some("src/main.cc".to_string()),
        );
        assert_eq!(
            layout.relativize(r"C:\work\probe\src\main.cc"),
            Some("src/main.cc".to_string()),
        );
        // Outside source dir → dropped.
        assert_eq!(
            layout.relativize("C:/packages/conda/include/iostream"),
            None,
        );
        // Inside the .pixi cache → dropped.
        assert_eq!(
            layout.relativize("C:/work/probe/.pixi/envs/default/include/foo.h"),
            None,
        );
    }

    #[test]
    fn relativize_drops_source_dir_itself() {
        let layout = layout("/src/proj", "/src/proj/build");
        assert_eq!(layout.relativize("/src/proj"), None);
        assert_eq!(layout.relativize("/src/proj/"), None);
    }

    #[test]
    fn verify_globs_extracts_glob_patterns_with_recursive_translation() {
        // Excerpt of the real VerifyGlobs.cmake format: one file(GLOB ...) call
        // per pattern, absolute path, with LIST_DIRECTORIES filler. CMake
        // splits multi-pattern user globs into separate file() calls.
        let text = r#"# CMAKE generated file: DO NOT EDIT!
# S1 at CMakeLists.txt:4 (file)
file(GLOB NEW_GLOB LIST_DIRECTORIES true "/src/proj/src/*.cc")
set(OLD_GLOB
  "/src/proj/src/a.cc"
  )
if(NOT "${NEW_GLOB}" STREQUAL "${OLD_GLOB}")
endif()

# S2 at CMakeLists.txt:5 (file)
file(GLOB NEW_GLOB LIST_DIRECTORIES true "/src/proj/include/*.hpp")

# Recursive: pixi matcher needs `**` injected.
file(GLOB_RECURSE NEW_GLOB LIST_DIRECTORIES false "/src/proj/include/*.h")

# Outside source dir: drop.
file(GLOB NEW_GLOB LIST_DIRECTORIES true "/opt/external/*.cc")

# Pixi cache: drop.
file(GLOB NEW_GLOB LIST_DIRECTORIES true "/src/proj/.pixi/envs/default/include/*.h")
"#;
        assert_eq!(
            collect_verify_globs(text, "/src/proj"),
            vec!["include/**/*.h", "include/*.hpp", "src/*.cc",]
        );
    }

    #[test]
    fn verify_globs_recursive_at_source_root_uses_double_star_prefix() {
        // `file(GLOB_RECURSE ... "<src_root>/*.h")`: no subdir component, so
        // we anchor the recursion with a `**/` prefix.
        let text = r#"file(GLOB_RECURSE NEW_GLOB LIST_DIRECTORIES false "/src/proj/*.h")"#;
        assert_eq!(collect_verify_globs(text, "/src/proj"), vec!["**/*.h"]);
    }

    #[test]
    fn verify_globs_preserves_user_double_star() {
        // If the user already wrote `**` we leave the pattern alone.
        let text =
            r#"file(GLOB_RECURSE NEW_GLOB LIST_DIRECTORIES false "/src/proj/include/**/*.hpp")"#;
        assert_eq!(
            collect_verify_globs(text, "/src/proj"),
            vec!["include/**/*.hpp"]
        );
    }

    #[test]
    fn verify_globs_ignores_non_file_glob_lines() {
        let text = r#"
# A comment line
set(OLD_GLOB
  "/src/proj/whatever"
  )
file(TOUCH_NOCREATE "/src/proj/build/CMakeFiles/cmake.verify_globs")
if(NOT "${NEW_GLOB}" STREQUAL "${OLD_GLOB}")
endif()
"#;
        assert!(collect_verify_globs(text, "/src/proj").is_empty());
    }

    #[test]
    fn pixi_cache_segment_recognizes_only_first_segment() {
        // First segment is `.pixi`: drop.
        assert!(is_pixi_cache_segment(".pixi"));
        assert!(is_pixi_cache_segment(".pixi/envs/default/lib"));
        // A directory that just happens to contain `.pixi` mid-path is fine.
        assert!(!is_pixi_cache_segment("docs/.pixi/foo"));
        assert!(!is_pixi_cache_segment("not_pixi/foo"));
    }

    #[test]
    fn canonicalize_resolves_dotdot_segments() {
        assert_eq!(canonicalize("foo/../bar", "/build"), "/build/bar");
        assert_eq!(
            canonicalize("../include/x.h", "/build/sub"),
            "/build/include/x.h"
        );
        assert_eq!(canonicalize("./x.h", "/build"), "/build/x.h");
        assert_eq!(canonicalize("/abs/path", "/ignored"), "/abs/path");
        assert_eq!(canonicalize(r"F:\abs\path", "/ignored"), "F:/abs/path");
    }

    #[test]
    fn is_absolute_recognizes_drive_letters_and_unix_root() {
        assert!(is_absolute("/foo"));
        assert!(is_absolute("F:/foo"));
        assert!(is_absolute(r"F:\foo"));
        assert!(!is_absolute("foo"));
        assert!(!is_absolute("../foo"));
    }
}
