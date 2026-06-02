from pathlib import Path
from mkdocs.structure.files import File, Files
from mkdocs.config.defaults import MkDocsConfig

HERE = Path(__file__).parent

COPY_PATHS = {
    HERE / "schema.json": "schema/manifest",
    HERE / "pyproject/schema.json": "schema/manifest",
    HERE / "pyproject/partial-pixi.json": "schema/manifest",
}


def on_files(files: Files, config: MkDocsConfig):
    """Copy the schemata to the site."""
    for path, dest in COPY_PATHS.items():
        files.append(
            File(
                path=path.relative_to(HERE).as_posix(),
                src_dir=HERE.as_posix(),
                dest_dir=f"{config.site_dir}/{dest}",
                use_directory_urls=config.use_directory_urls,
            )
        )
    return files
