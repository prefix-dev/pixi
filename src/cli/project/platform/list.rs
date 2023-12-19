use crate::Project;

pub async fn execute(project: Project) -> miette::Result<()> {
    project.platforms().iter().for_each(|platform| {
        println!("{}", platform.as_str());
    });

    Ok(())
}
