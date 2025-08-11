use std::fmt::Display;

use thiserror::Error;

#[derive(Debug, Error)]
pub struct EditablePackagesMismatch {
    pub expected_editable: Vec<uv_normalize::PackageName>,
    pub unexpected_editable: Vec<uv_normalize::PackageName>,
}

impl Display for EditablePackagesMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if !self.expected_editable.is_empty() && self.unexpected_editable.is_empty() {
            write!(f, "expected ")?;
            format_package_list(f, &self.expected_editable)?;
            write!(
                f,
                " to be editable but in the lock-file {they} {are} not",
                they = it_they(self.expected_editable.len()),
                are = is_are(self.expected_editable.len())
            )?
        } else if self.expected_editable.is_empty() && !self.unexpected_editable.is_empty() {
            write!(f, "expected ")?;
            format_package_list(f, &self.unexpected_editable)?;
            write!(
                f,
                "NOT to be editable but in the lock-file {they} {are}",
                they = it_they(self.unexpected_editable.len()),
                are = is_are(self.unexpected_editable.len())
            )?
        } else {
            write!(f, "expected ")?;
            format_package_list(f, &self.expected_editable)?;
            write!(
                f,
                " to be editable but in the lock-file but {they} {are} not, whereas ",
                they = it_they(self.expected_editable.len()),
                are = is_are(self.expected_editable.len())
            )?;
            format_package_list(f, &self.unexpected_editable)?;
            write!(
                f,
                " {are} NOT expected to be editable which in the lock-file {they} {are}",
                they = it_they(self.unexpected_editable.len()),
                are = is_are(self.unexpected_editable.len())
            )?
        }

        return Ok(());

        fn format_package_list(
            f: &mut std::fmt::Formatter<'_>,
            packages: &[uv_normalize::PackageName],
        ) -> std::fmt::Result {
            for (idx, package) in packages.iter().enumerate() {
                if idx == packages.len() - 1 && idx > 0 {
                    write!(f, " and ")?;
                } else if idx > 0 {
                    write!(f, ", ")?;
                }
                write!(f, "{}", package)?;
            }

            Ok(())
        }

        fn is_are(count: usize) -> &'static str {
            if count == 1 { "is" } else { "are" }
        }

        fn it_they(count: usize) -> &'static str {
            if count == 1 { "it" } else { "they" }
        }
    }
}
