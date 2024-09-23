use std::{
    fs::File,
    io::{BufRead, Write},
    path::Path,
};

/// A function that loads the globs from the `conda-meta` directory.
/// These are used for checking if a re-install of a package is needed.
pub fn load_reinstall_globs(glob_file: &Path) -> Result<Option<Vec<String>>, std::io::Error> {
    // Load the globs from the file
    // Text file with one glob per line
    let globs = File::open(glob_file);
    let globs = match globs {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
        Ok(file) => file,
    };
    let globs = std::io::BufReader::new(globs)
        .lines()
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(globs))
}

/// Write the re-install globs to the given file.
pub fn write_reinstall_globs(glob_file: &Path, globs: &[String]) -> Result<(), std::io::Error> {
    // Write the globs to the file
    let mut file = File::create(glob_file)?;
    for glob in globs {
        writeln!(&mut file, "{}", glob)?;
    }
    Ok(())
}
