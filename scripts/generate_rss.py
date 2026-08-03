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


# ---------------------------------------------------------------------------
# Tests — run via: uv run --with feedgen --with pytest pytest scripts/generate_rss.py
# ---------------------------------------------------------------------------

import pytest  # noqa: E402


def test_parse_changelog_extracts_versions():
    text = """\
# Changelog

### [0.2.0] - 2024-03-15

#### Added

- Feature A

### [0.1.0] - 2024-01-01

#### Fixed

- Bug fix
"""
    entries = parse_changelog(text)
    assert len(entries) == 2
    assert entries[0]["version"] == "0.2.0"
    assert entries[0]["date"] == date(2024, 3, 15)
    assert "Feature A" in entries[0]["body"]
    assert entries[1]["version"] == "0.1.0"
    assert entries[1]["date"] == date(2024, 1, 1)
    assert "Bug fix" in entries[1]["body"]


def test_parse_changelog_no_versions():
    assert parse_changelog("# Changelog\n\nNothing here.\n") == []


def test_parse_changelog_invalid_date_exits():
    text = "### [0.1.0] - 2024-13-01\n\ncontent\n"
    with pytest.raises(SystemExit) as exc_info:
        parse_changelog(text)
    msg = str(exc_info.value)
    assert "2024-13-01" in msg
    assert "0.1.0" in msg
    assert "YYYY-MM-DD" in msg


def test_build_feed_returns_rss_xml():
    entries = [
        {"version": "0.2.0", "date": date(2024, 3, 15), "body": "## Added\n\n- Feature A"},
        {"version": "0.1.0", "date": date(2024, 1, 1), "body": "## Fixed\n\n- Bug fix"},
    ]
    xml = build_feed(entries, site_url="https://example.com")
    assert b"<rss" in xml
    assert b"pixi v0.2.0" in xml
    assert b"pixi v0.1.0" in xml
    assert b"Feature A" in xml


def test_build_feed_includes_canonical_self_link():
    entries = [{"version": "0.1.0", "date": date(2024, 1, 1), "body": "content"}]
    xml = build_feed(entries, site_url="https://example.com")
    assert b"example.com/feed.xml" in xml


def test_build_feed_item_links_to_changelog_anchor():
    entries = [{"version": "0.71.1", "date": date(2026, 6, 25), "body": "content"}]
    xml = build_feed(entries, site_url="https://example.com")
    # Anchor derived from heading text "[0.71.1] - 2026-06-25":
    # brackets/dots stripped -> "0711 - 2026-06-25" -> slugified -> "0711-2026-06-25"
    # If links produce 404s after deploy, verify against a built Zensical page and
    # update _anchor() accordingly.
    assert b"CHANGELOG/#0711-2026-06-25" in xml


def test_build_feed_published_date():
    entries = [{"version": "0.1.0", "date": date(2024, 3, 15), "body": "content"}]
    xml = build_feed(entries, site_url="https://example.com")
    assert b"2024-03-15" in xml


if __name__ == "__main__":
    main()
