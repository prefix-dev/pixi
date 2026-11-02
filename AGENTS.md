- You MUST NOT manually edit `CHANGELOG.md` outside the release workflow; it is generated.

## Tooling

- PREFER to use `pixi run` to execute tasks instead of running executables coming from your system.

## Testing

- PREFER inline snapshots over substring assertions when testing rendered output.
- New tests MUST follow local test conventions. Check existing coverage before adding tests.
- PREFER integration tests for user-visible behavior; use unit tests for isolated logic.
- PREFER running specific tests during development; run relevant broader checks before finishing.

## Dependencies

- You MUST NOT work around limitations or bugs in rattler or rattler-build. Fix them upstream instead.

## Linting

- `pixi run typos` MUST pass. Suppress deliberate misspellings with `# spellchecker:off` / `# spellchecker:on` or their `//` equivalents instead of adding them to `extend-words`.
- You MUST NOT bypass pre-commit hooks with `--no-verify`.

## Rust

- Use enums with variant data to represent mutually exclusive states instead of contradictory flags or dependent `Option` fields.

## Pull requests

- Pull requests MUST follow [CONTRIBUTING.md](CONTRIBUTING.md) and the [pull request template](.github/pull_request_template.md), including the human-written description requirement.

## Error messages

- Authored diagnostic messages MUST start with a lowercase letter and end without a trailing dot. This does not apply to quoted upstream errors.
