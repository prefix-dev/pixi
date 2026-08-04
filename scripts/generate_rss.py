"""Generate Atom feed from CHANGELOG.md.

Parses version headings of the form '### [X.Y.Z] - YYYY-MM-DD', builds an
Atom feed with lxml, and writes docs/feed.xml so that Zensical includes
it in the built site.
"""

import re
import sys
from datetime import date, datetime, timezone
from pathlib import Path

from lxml import etree

REPO_ROOT = Path(__file__).parent.parent
CHANGELOG = REPO_ROOT / "CHANGELOG.md"
OUTPUT = REPO_ROOT / "docs" / "feed.xml"

SITE_URL = "https://pixi.prefix.dev/latest"
ATOM_NS = "http://www.w3.org/2005/Atom"

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
    """Build and return Atom XML bytes from parsed changelog entries."""
    feed = etree.Element("feed", nsmap={None: ATOM_NS})  # ty: ignore[invalid-argument-type]

    title = etree.SubElement(feed, "title")
    title.text = "pixi Changelog"

    link_alt = etree.SubElement(feed, "link")
    link_alt.set("rel", "alternate")
    link_alt.set("href", f"{site_url}/CHANGELOG/")

    link_self = etree.SubElement(feed, "link")
    link_self.set("rel", "self")
    link_self.set("href", f"{site_url}/feed.xml")

    feed_id = etree.SubElement(feed, "id")
    feed_id.text = f"{site_url}/CHANGELOG/"

    subtitle = etree.SubElement(feed, "subtitle")
    subtitle.text = "Release notes for pixi, a fast cross-platform package manager"

    if entries:
        latest = entries[0]["date"]
        latest_dt = datetime(latest.year, latest.month, latest.day, tzinfo=timezone.utc)
        updated = etree.SubElement(feed, "updated")
        updated.text = latest_dt.strftime("%Y-%m-%dT%H:%M:%SZ")

    for entry in entries:
        entry_el = etree.SubElement(feed, "entry")
        anchor = _anchor(entry["version"], entry["date"])

        entry_id = etree.SubElement(entry_el, "id")
        entry_id.text = f"{site_url}/CHANGELOG/#{anchor}"

        entry_title = etree.SubElement(entry_el, "title")
        entry_title.text = f"pixi v{entry['version']}"

        entry_link = etree.SubElement(entry_el, "link")
        entry_link.set("href", f"{site_url}/CHANGELOG/#{anchor}")

        published_dt = datetime(
            entry["date"].year,
            entry["date"].month,
            entry["date"].day,
            tzinfo=timezone.utc,
        )
        dt_str = published_dt.strftime("%Y-%m-%dT%H:%M:%SZ")

        published_el = etree.SubElement(entry_el, "published")
        published_el.text = dt_str

        updated_el = etree.SubElement(entry_el, "updated")
        updated_el.text = dt_str

        content_el = etree.SubElement(entry_el, "content")
        content_el.set("type", "text")
        content_el.text = entry["body"]

    return etree.tostring(feed, pretty_print=True, xml_declaration=True, encoding="UTF-8")


def main() -> None:
    text = CHANGELOG.read_text(encoding="utf-8")
    entries = parse_changelog(text)
    xml = build_feed(entries)
    OUTPUT.write_bytes(xml)
    print(f"Written {len(entries)} entries to {OUTPUT}")


if __name__ == "__main__":
    main()
