use crate::Project;

pub async fn execute(project: Project) -> miette::Result<()> {
    println!("{}", project.name());
    Ok(())
}
