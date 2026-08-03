"""Generate RSS 2.0 feed from CHANGELOG.md.

Parses version headings of the form '### [X.Y.Z] - YYYY-MM-DD', builds an
RSS 2.0 feed with feedgen, and writes docs/feed.xml so that Zensical includes
it in the built site.

Run via:  pixi run generate-rss
Test via: pixi run test-rss
"""

import re
import sys
from datetime import date, datetime, timezone
from pathlib import Path

from feedgen.feed import FeedGenerator

REPO_ROOT = Path(__file__).parent.parent
CHANGELOG = REPO_ROOT / "CHANGELOG.md"
OUTPUT = REPO_ROOT / "docs" / "feed.xml"

SITE_URL = "https://pixi.prefix.dev/latest"

_HEADING_RE = re.compile(
    r"^### \[(\d+\.\d+\.\d+)\] - (\d{4}-\d{2}-\d{2})$",
    re.MULTILINE,
)


def parse_changelog(text: str) -> list[dict]:
    """Return a list of {version, date, body} dicts, newest first.

    Raises SystemExit if a heading contains a date that cannot be parsed as
    YYYY-MM-DD (e.g. month 13, or a non-numeric value).
    """
    entries = []
    matches = list(_HEADING_RE.finditer(text))
    for i, match in enumerate(matches):
        version = match.group(1)
        date_str = match.group(2)
        try:
            entry_date = datetime.strptime(date_str, "%Y-%m-%d").date()
        except ValueError:
            sys.exit(
                f"Error: invalid date {date_str!r} in changelog heading "
                f"'### [{version}] - {date_str}'\n"
                f"Expected format: YYYY-MM-DD"
            )
        body_start = match.end()
        body_end = matches[i + 1].start() if i + 1 < len(matches) else len(text)
        body = text[body_start:body_end].strip()
        entries.append({"version": version, "date": entry_date, "body": body})
    return entries


def _anchor(version: str, entry_date: date) -> str:
    """Return the URL fragment Zensical generates for a version heading.

    Zensical slugifies the heading text '[X.Y.Z] - YYYY-MM-DD' by:
      1. Removing characters that are not word chars, spaces, or hyphens
      2. Lowercasing
      3. Collapsing runs of spaces and hyphens to a single hyphen

    Example: '[0.71.1] - 2026-06-25' -> '0711-2026-06-25'

    If changelog links produce 404s after deploy, verify by inspecting a
    Zensical-built CHANGELOG/index.html and update this function.
    """
    heading_text = f"[{version}] - {entry_date.strftime('%Y-%m-%d')}"
    slug = re.sub(r"[^\w\s-]", "", heading_text).lower()
    slug = re.sub(r"[-\s]+", "-", slug).strip("-")
    return slug


def build_feed(entries: list[dict], *, site_url: str = SITE_URL) -> bytes:
    """Build and return RSS 2.0 XML bytes from parsed changelog entries."""
    fg = FeedGenerator()
    fg.id(f"{site_url}/CHANGELOG/")
    fg.title("pixi Changelog")
    fg.link(href=f"{site_url}/CHANGELOG/", rel="alternate")
    fg.link(href=f"{site_url}/feed.xml", rel="self")
    fg.description("Release notes for pixi, a fast cross-platform package manager")
    fg.language("en")

    for entry in entries:
        fe = fg.add_entry()
        anchor = _anchor(entry["version"], entry["date"])
        fe.id(f"{site_url}/CHANGELOG/#{anchor}")
        fe.title(f"pixi v{entry['version']}")
        fe.link(href=f"{site_url}/CHANGELOG/#{anchor}")
        published = datetime(
            entry["date"].year,
            entry["date"].month,
            entry["date"].day,
            tzinfo=timezone.utc,
        )
        fe.published(published)
        fe.updated(published)
        fe.content(entry["body"], type="text")

    return fg.rss_str(pretty=True)


def main() -> None:
    text = CHANGELOG.read_text(encoding="utf-8")
    entries = parse_changelog(text)
    xml = build_feed(entries)
    OUTPUT.write_bytes(xml)
    print(f"Written {len(entries)} entries to {OUTPUT}")

if __name__ == "__main__":
    main()
