//! Per-language templates for initializing a `conda-script` file.
//!
//! Each template carries the comment prefix of the language, an entrypoint
//! that runs the file straight away, the toolchain packages the entrypoint
//! needs, and a starter body for a freshly created file.

/// A per-language `conda-script` starting point for `pixi init`.
pub struct CondaScriptTemplate {
    /// The file extensions the template covers, lowercase.
    pub extensions: &'static [&'static str],
    /// The comment prefix of the language, including the trailing space.
    pub prefix: &'static str,
    /// The channels of the generated block.
    pub channels: &'static [&'static str],
    /// The entrypoint of the generated block.
    pub entrypoint: &'static str,
    /// The toolchain dependencies of the generated block.
    pub dependencies: &'static [&'static str],
    /// The program a freshly created file starts out with.
    pub body: &'static str,
}

const CONDA_FORGE: &[&str] = &["conda-forge"];

const TEMPLATES: &[CondaScriptTemplate] = &[
    CondaScriptTemplate {
        extensions: &["py", "pyw"],
        prefix: "# ",
        channels: CONDA_FORGE,
        entrypoint: "python ${SCRIPT}",
        dependencies: &["python"],
        body: "print(\"Hello from pixi!\")\n",
    },
    CondaScriptTemplate {
        extensions: &["c"],
        prefix: "// ",
        channels: CONDA_FORGE,
        entrypoint: "gcc -o ${CACHE}/main ${SCRIPT} && ${CACHE}/main",
        dependencies: &["gcc"],
        body: "#include <stdio.h>\n\nint main(void) {\n    printf(\"Hello from pixi!\\n\");\n    return 0;\n}\n",
    },
    CondaScriptTemplate {
        extensions: &["cpp", "cc", "cxx"],
        prefix: "// ",
        channels: CONDA_FORGE,
        entrypoint: "g++ -o ${CACHE}/main ${SCRIPT} && ${CACHE}/main",
        dependencies: &["gxx"],
        body: "#include <iostream>\n\nint main() {\n    std::cout << \"Hello from pixi!\\n\";\n    return 0;\n}\n",
    },
    CondaScriptTemplate {
        extensions: &["cs"],
        prefix: "// ",
        channels: CONDA_FORGE,
        entrypoint: "dotnet run ${SCRIPT}",
        dependencies: &["dotnet"],
        body: "Console.WriteLine(\"Hello from pixi!\");\n",
    },
    CondaScriptTemplate {
        extensions: &["go"],
        prefix: "// ",
        channels: CONDA_FORGE,
        entrypoint: "go run ${SCRIPT}",
        dependencies: &["go"],
        body: "package main\n\nimport \"fmt\"\n\nfunc main() {\n\tfmt.Println(\"Hello from pixi!\")\n}\n",
    },
    CondaScriptTemplate {
        extensions: &["f90"],
        prefix: "! ",
        channels: CONDA_FORGE,
        entrypoint: "gfortran -o ${CACHE}/main ${SCRIPT} && ${CACHE}/main",
        dependencies: &["gfortran"],
        body: "program main\n  print *, \"Hello from pixi!\"\nend program main\n",
    },
    CondaScriptTemplate {
        extensions: &["pl"],
        prefix: "# ",
        channels: CONDA_FORGE,
        entrypoint: "perl ${SCRIPT}",
        dependencies: &["perl"],
        body: "print \"Hello from pixi!\\n\";\n",
    },
    CondaScriptTemplate {
        extensions: &["r"],
        prefix: "# ",
        channels: CONDA_FORGE,
        entrypoint: "Rscript ${SCRIPT}",
        dependencies: &["r-base"],
        body: "cat(\"Hello from pixi!\\n\")\n",
    },
    CondaScriptTemplate {
        extensions: &["rs"],
        prefix: "// ",
        channels: CONDA_FORGE,
        entrypoint: "rustc -o ${CACHE}/main ${SCRIPT} -C linker=gcc && ${CACHE}/main",
        dependencies: &["rust", "gcc"],
        body: "fn main() {\n    println!(\"Hello from pixi!\");\n}\n",
    },
    CondaScriptTemplate {
        extensions: &["kts"],
        prefix: "// ",
        channels: CONDA_FORGE,
        entrypoint: "kotlin ${SCRIPT}",
        dependencies: &["kotlin"],
        body: "println(\"Hello from pixi!\")\n",
    },
    CondaScriptTemplate {
        extensions: &["rb"],
        prefix: "# ",
        channels: CONDA_FORGE,
        entrypoint: "ruby ${SCRIPT}",
        dependencies: &["ruby"],
        body: "puts \"Hello from pixi!\"\n",
    },
    CondaScriptTemplate {
        extensions: &["jl"],
        prefix: "# ",
        channels: CONDA_FORGE,
        entrypoint: "julia ${SCRIPT}",
        dependencies: &["julia"],
        body: "println(\"Hello from pixi!\")\n",
    },
    CondaScriptTemplate {
        extensions: &["ts"],
        prefix: "// ",
        channels: CONDA_FORGE,
        entrypoint: "deno run ${SCRIPT}",
        dependencies: &["deno"],
        body: "console.log(\"Hello from pixi!\");\n",
    },
    // The conda-forge `scala3` package reports a `-bin-SNAPSHOT` compiler
    // version that Maven cannot resolve, so the entrypoint pins the real
    // compiler release; keep the two versions in sync.
    CondaScriptTemplate {
        extensions: &["scala"],
        prefix: "// ",
        channels: CONDA_FORGE,
        entrypoint: "scala run ${SCRIPT} --workspace ${CACHE} --scala-version 3.7.4",
        dependencies: &["scala3"],
        body: "@main def hello(): Unit = println(\"Hello from pixi!\")\n",
    },
    CondaScriptTemplate {
        extensions: &["lua"],
        prefix: "-- ",
        channels: CONDA_FORGE,
        entrypoint: "lua ${SCRIPT}",
        dependencies: &["lua"],
        body: "print(\"Hello from pixi!\")\n",
    },
    CondaScriptTemplate {
        extensions: &["sh"],
        prefix: "# ",
        channels: CONDA_FORGE,
        entrypoint: "brush ${SCRIPT}",
        dependencies: &["brush"],
        body: "echo \"Hello from pixi!\"\n",
    },
    CondaScriptTemplate {
        extensions: &["ps1"],
        prefix: "# ",
        channels: CONDA_FORGE,
        entrypoint: "pwsh -NoProfile -File ${SCRIPT}",
        dependencies: &["powershell"],
        body: "Write-Output \"Hello from pixi!\"\n",
    },
    CondaScriptTemplate {
        extensions: &["zig"],
        prefix: "// ",
        channels: CONDA_FORGE,
        entrypoint: "zig run ${SCRIPT}",
        dependencies: &["zig"],
        body: "const std = @import(\"std\");\n\npub fn main() void {\n    std.debug.print(\"Hello from pixi!\\n\", .{});\n}\n",
    },
    CondaScriptTemplate {
        extensions: &["mojo"],
        prefix: "# ",
        channels: &["https://conda.modular.com/max", "conda-forge"],
        entrypoint: "mojo ${SCRIPT}",
        dependencies: &["mojo"],
        body: "def main():\n    print(\"Hello from pixi!\")\n",
    },
];

/// The template for a file extension, matched case-insensitively.
pub fn template_for_extension(extension: &str) -> Option<&'static CondaScriptTemplate> {
    let extension = extension.to_ascii_lowercase();
    TEMPLATES
        .iter()
        .find(|template| template.extensions.contains(&extension.as_str()))
}

/// Every extension a template exists for, in listing order.
pub fn supported_extensions() -> Vec<&'static str> {
    TEMPLATES
        .iter()
        .flat_map(|template| template.extensions.iter().copied())
        .collect()
}
