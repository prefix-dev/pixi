use std::fmt::{self, Display, Formatter};

use itertools::Itertools;
use rattler_conda_types::PackageName;

/// Defines a package and in which dependency set the cycle occurred.
#[derive(Debug)]
pub enum CycleEnvironment {
    Host,
    Build,
    Run,
}

#[derive(Debug, Default)]
pub struct Cycle {
    /// A list of package and in which environment the next package is used.
    /// Together, these form a cycle.
    pub stack: Vec<(PackageName, CycleEnvironment)>,
}

impl Display for CycleEnvironment {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            CycleEnvironment::Host => write!(f, "host"),
            CycleEnvironment::Build => write!(f, "build"),
            CycleEnvironment::Run => write!(f, "run"),
        }
    }
}

impl Display for Cycle {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if self.stack.is_empty() {
            return writeln!(f, "Empty cycle detected");
        }

        // Top border
        writeln!(f, "┌──→──┐")?;

        // Display each step in the cycle
        for (i, ((from_package, env_type), (to_package, _))) in
            self.stack.iter().rev().circular_tuple_windows().enumerate()
        {
            // Show the package that declares the dependency
            writeln!(f, "|  {}", from_package.as_source())?;
            writeln!(f, "|    requires {} ({})", to_package.as_source(), env_type)?;

            // Add flow arrows except for the last item
            if i < self.stack.len() - 1 {
                writeln!(f, "↑     ↓")?;
            }
        }

        // Bottom border
        writeln!(f, "└──←──┘")?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cycle_display() {
        let cycle = Cycle {
            stack: vec![
                ("package_d".parse().unwrap(), CycleEnvironment::Host),
                ("package_c".parse().unwrap(), CycleEnvironment::Run),
                ("package_b".parse().unwrap(), CycleEnvironment::Build),
                ("package_a".parse().unwrap(), CycleEnvironment::Host),
            ],
        };

        insta::assert_snapshot!(cycle.to_string());
    }
}
