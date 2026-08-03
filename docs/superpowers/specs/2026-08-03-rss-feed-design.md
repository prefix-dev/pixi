# RSS Feed for Pixi Changelog — Design Spec

**Date:** 2026-08-03

## Overview

Generate an RSS 2.0 feed from `CHANGELOG.md` so users can subscribe to pixi release notes. The feed is generated as a pre-build step in the docs pipeline and served as a static file via MkDocs.

## Architecture

A standalone Python script (`scripts/generate_rss.py`) parses `CHANGELOG.md`, builds a feed with `feedgen`, and writes `docs/feed.xml`. MkDocs copies any non-markdown file from `docs/` into `site/` verbatim, so the feed becomes available at `pixi.prefix.dev/<version>/feed.xml` after each mike deploy.

A new pixi task `generate-rss` runs the script, and is added as a `depends-on` entry for both the `build-docs` and `docs` tasks in `pixi.toml`.

## Script: `scripts/generate_rss.py`

### Changelog parsing

- Regex splits `CHANGELOG.md` on headings of the form `### [X.Y.Z] - YYYY-MM-DD`
- Each match yields: version string (e.g. `0.71.1`), date string (e.g. `2026-06-25`), body text (everything until the next heading)
- Date is parsed with `datetime.strptime(date_str, "%Y-%m-%d")`; on failure a clear `SystemExit` is raised:
  ```
  Error: invalid date "2026-13-01" in changelog heading "### [0.71.1] - 2026-13-01"
  Expected format: YYYY-MM-DD
  ```
- The script errors on the first invalid heading and exits non-zero, which will fail the pixi task and block the docs build

### Feed construction (feedgen)

- **Channel metadata:**
  - title: `pixi Changelog`
  - link: `https://pixi.prefix.dev/latest/CHANGELOG/`
  - description: `Release notes for pixi, a fast cross-platform package manager`
  - language: `en`
  - `<atom:link rel="self">` pointing to `https://pixi.prefix.dev/latest/feed.xml` (canonical URL convention)
- **Per-entry:**
  - id: `https://pixi.prefix.dev/latest/CHANGELOG/#<anchor>` where anchor is the version with dots replaced by empty string, lowercased (e.g. `#0711` for `0.71.1`) — matches MkDocs Material's auto-generated heading anchors
  - title: `pixi v<version>`
  - published/updated: parsed date at midnight UTC
  - content: raw markdown body of that changelog section
- All entries included (no artificial limit)

### Output

- Written to `docs/feed.xml`
- `docs/feed.xml` is added to `.gitignore` (it is a build artifact, not source)

## pixi.toml changes

```toml
[feature.docs.dependencies]
feedgen = ">=0.9,<1"
# ... existing deps

[feature.docs.tasks]
generate-rss = { cmd = "python scripts/generate_rss.py", description = "Generate RSS feed from changelog" }
build-docs = { cmd = "mkdocs build --strict", depends-on = [
  "download-font",
  "generate-rss",
], description = "Build documentation" }
docs = { cmd = "mkdocs serve", depends-on = [
  "download-font",
  "generate-rss",
], description = "Serve the docs locally" }
```

## Versioning and mike

| URL | Updated when | Suitable for subscription |
|-----|-------------|--------------------------|
| `pixi.prefix.dev/latest/feed.xml` | Every release tag | Yes — canonical URL |
| `pixi.prefix.dev/dev/feed.xml` | Every push to `main` | Noisier, may include pre-release entries |
| `pixi.prefix.dev/v0.74.0/feed.xml` | Never (frozen) | No |

The feed includes `<atom:link rel="self">` pointing at the `latest` URL as a hint to feed readers. No active redirect mechanism exists in RSS; the canonical URL should be documented on the changelog page.

## Out of scope

- Server-side redirect from `/dev/feed.xml` to `/latest/feed.xml`
- Rendered HTML descriptions (raw markdown used instead)
- Limiting feed to N most recent entries
