use std::fmt::{self, Display, Formatter};

use rattler_conda_types::PackageName;

#[derive(Debug)]
pub enum CycleEnvironment {
    Host(PackageName),
    Build(PackageName),
    Run(PackageName),
}

#[derive(Debug, Default)]
pub struct Cycle {
    pub stack: Vec<CycleEnvironment>,
}

impl CycleEnvironment {
    pub fn package_name(&self) -> &PackageName {
        match self {
            CycleEnvironment::Host(name) => name,
            CycleEnvironment::Build(name) => name,
            CycleEnvironment::Run(name) => name,
        }
    }

    pub fn dependency_type(&self) -> &str {
        match self {
            CycleEnvironment::Host(_) => "host",
            CycleEnvironment::Build(_) => "build",
            CycleEnvironment::Run(_) => "run",
        }
    }

    pub fn with_package_name(self, package_name: PackageName) -> Self {
        match self {
            CycleEnvironment::Host(_) => CycleEnvironment::Host(package_name),
            CycleEnvironment::Build(_) => CycleEnvironment::Build(package_name),
            CycleEnvironment::Run(_) => CycleEnvironment::Run(package_name),
        }
    }
}

impl Display for CycleEnvironment {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} ({} environment)",
            self.package_name().as_source(),
            self.dependency_type()
        )
    }
}

impl Display for Cycle {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if self.stack.is_empty() {
            return writeln!(f, "Empty cycle detected");
        }

        // Top border
        writeln!(f, "┌─────┐")?;

        // Display each step in the cycle
        for (i, dep) in self.stack.iter().enumerate().rev() {
            let next_idx = (i + self.stack.len() - 1) % self.stack.len();
            let next_dep = &self.stack[next_idx];
            let from_package = dep.package_name();
            let env_type = dep.dependency_type();
            let to_package = next_dep.package_name();

            // Show the package that declares the dependency
            writeln!(f, "|  {}", from_package.as_source())?;
            writeln!(f, "|    requires {} ({})", to_package.as_source(), env_type)?;

            // Add flow arrows except for the last item
            if i > 0 {
                writeln!(f, "↑     ↓")?;
            }
        }

        // Bottom border
        writeln!(f, "└─────┘")?;

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
                CycleEnvironment::Host("package_d".parse().unwrap()),
                CycleEnvironment::Run("package_c".parse().unwrap()),
                CycleEnvironment::Build("package_b".parse().unwrap()),
                CycleEnvironment::Host("package_a".parse().unwrap()),
            ],
        };

        insta::assert_snapshot!(cycle.to_string());
    }
}
