from pathlib import Path
from mkdocs.structure.files import File, Files
from mkdocs.config.defaults import MkDocsConfig

INSTALL_SCRIPTS = [Path(__file__).parent / "install.sh", Path(__file__).parent / "install.ps1"]


def on_files(files: Files, config: MkDocsConfig):
    """Copy the installation scripts to the site."""
    for script in INSTALL_SCRIPTS:
        files.append(
            File(
                path=script.name,
                src_dir=script.parent,
                dest_dir=f"{config.site_dir}",
                use_directory_urls=config.use_directory_urls,
            )
        )
    return files
