use std::io::Write;

use human_bytes::human_bytes;
use rattler_conda_types::{PackageName, PackageRecord, Version};
use serde::Serialize;

#[derive(Serialize, Hash, Eq, PartialEq)]
pub struct PackageToOutput {
    pub name: PackageName,
    version: Version,
    build: Option<String>,
    pub size_bytes: Option<u64>,
    is_explicit: bool,
}

impl PackageToOutput {
    pub fn new(record: &PackageRecord, is_explicit: bool) -> Self {
        Self {
            name: record.name.clone(),
            version: record.version.version().clone(),
            build: Some(record.build.clone()),
            size_bytes: record.size,
            is_explicit,
        }
    }
}

/// Create a human-readable representation of a list of packages.
/// Using a tabwriter to align the columns.
pub fn print_package_table(packages: Vec<PackageToOutput>) -> Result<(), std::io::Error> {
    let mut writer = tabwriter::TabWriter::new(std::io::stdout());
    let header_style = console::Style::new().bold().cyan();
    let header = format!(
        "{}\t{}\t{}\t{}",
        header_style.apply_to("Package"),
        header_style.apply_to("Version"),
        header_style.apply_to("Build"),
        header_style.apply_to("Size"),
    );
    writeln!(writer, "{}", &header)?;

    for package in packages {
        // Convert size to human-readable format
        let size_human = package
            .size_bytes
            .map(|size| human_bytes(size as f64))
            .unwrap_or_default();

        let package_info = format!(
            "{}\t{}\t{}\t{}",
            package.name.as_normalized(),
            &package.version,
            package.build.as_deref().unwrap_or(""),
            size_human
        );

        writeln!(
            writer,
            "{}",
            if package.is_explicit {
                console::style(package_info).green().to_string()
            } else {
                package_info
            }
        )?;
    }

    writeln!(writer, "{}\n", header)?;
    writer.flush()
}
