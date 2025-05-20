use crate::common::PixiControl;

use rattler_conda_types::Platform;

#[tokio::test]
async fn test_pypi_conflict_resolution() {
    let platform = Platform::current();

    let pixi = PixiControl::from_manifest(&format!(
        r#"
        [project]
        name = "testo"
        version = "0.1.0"
        channels = ["conda-forge"]
        platforms = ["{platform}"]

        [dependencies]
        gdal = "*"
        python = ">=3.13.3,<3.14"

        [pypi-dependencies]
        numpy = "<2.2"
        "#,
    ))
    .unwrap();

    let update_err = pixi
        .update_lock_file()
        .await
        .expect_err("expected lock file update to fail due to PyPI dependency conflict")
        .display_without_colors();

    let expected_err = format!(
        r#"  × failed to solve the pypi requirements of 'default' '{platform}'
  ├─▶ failed to resolve pypi dependencies
  ├─▶ Because you require numpy<2.2 and numpy==2.2.6, we can conclude that your requirements are unsatisfiable.
  ╰─▶ numpy==2.2.6 is required by gdal
"#
    );

    assert_eq!(
        update_err, expected_err,
        "\n{update_err}\nvs\n\n{expected_err}"
    );
}

trait ReportExt {
    fn display_without_colors(self) -> String;
}

impl ReportExt for miette::Report {
    fn display_without_colors(self) -> String {
        use miette::{Diagnostic, GraphicalReportHandler, GraphicalTheme};

        let mut s = String::new();
        GraphicalReportHandler::new()
            .with_theme(GraphicalTheme::unicode_nocolor())
            .render_report(&mut s, &*Box::<dyn Diagnostic>::from(self))
            .unwrap();
        s
    }
}
