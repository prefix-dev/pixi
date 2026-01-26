"""This is a little script to generate a custom pypi simple index from a directory of source packages."""

from pathlib import Path
import shutil
from build import ProjectBuilder

indexes_path = Path(__file__).parent

index_html_template = """\
<!-- This file has been generated with `generate-indexes.py` -->
<!DOCTYPE html>
<html>
  <body>
    %LINKS%
  </body>
</html>
"""

for index_path in indexes_path.iterdir():
    if not index_path.is_dir():
        continue

    print(index_path)

    flat_index = index_path / "flat"
    shutil.rmtree(flat_index)
    flat_index.mkdir(exist_ok=True)

    wheels: list[Path] = []
    for package in (index_path / "src").iterdir():
        wheels.append(Path(ProjectBuilder(package).build("sdist", flat_index)))
        wheels.append(Path(ProjectBuilder(package).build("wheel", flat_index)))

    index = index_path / "index"
    shutil.rmtree(index)
    index.mkdir(exist_ok=True)

    projects: dict[str, list[Path]] = {}
    for wheel in wheels:
        project = wheel.name.split("-")[0]
        wheel_list = projects[project] = projects.get(project, [])
        wheel_list.append(wheel)

    for project, wheels in projects.items():
        index_dir = index / project
        index_dir.mkdir(exist_ok=True)

        for wheel in wheels:
            (index_dir / wheel.name).hardlink_to(wheel)

        (index_dir / "index.html").write_text(
            index_html_template.replace(
                "%LINKS%", "\n".join(f'<a href="{wheel.name}">{wheel.name}</a>' for wheel in wheels)
            )
        )

    (index / "index.html").write_text(
        index_html_template.replace(
            "%LINKS%",
            "\n".join(f'<a href="/{project}">{project}</a>' for project in projects.keys()),
        )
    )
