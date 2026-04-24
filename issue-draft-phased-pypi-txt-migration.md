# [Feature] First-class `requirements.txt` references in the manifest (`pypi-txt`) for phased migration

## Summary

Allow the Pixi manifest to **reference existing pip `requirements.txt` files** (including transitive `-r` includes) so PyPI dependencies stay **defined only in those files** while the rest of the stack moves to Pixi incrementally. This is **feature parity with conda**: the same “delegate PyPI pins to a file” workflow users already have in `environment.yml`, expressed in Pixi’s manifest instead.

## Motivation / use case

Many teams want a **phased migration** to Pixi:

- They are **not ready** to move all PyPI pins and optional stacks into `[pypi-dependencies]` (or `pyproject.toml`) in one step.
- Duplicating the same constraints in **both** `requirements.txt` **and** `pixi.toml` creates **two sources of truth**, drift, and review overhead.
- Existing workflows (CI, other tools, IDEs, `pip install -r`) may still consume the `.txt` files for a while.

**Conda analogue:** In `environment.yml`, `dependencies` can mix conda specs with a `pip:` list, and that list may include **`-r requirements.txt`** so all pip constraints live in a separate file while conda deps stay in YAML. This request is for a **similar capability in Pixi**—a manifest-level hook that says “also take PyPI requirements from these paths,” so teams get the **same phased migration story** they are used to from conda/mamba without maintaining a second copy of every line in TOML.

Today, `pixi import --format=pypi-txt` helps **one-time** conversion, but ongoing projects need the **manifest to point at** the canonical requirements files so Pixi resolves and locks against **the same lines** the team already maintains.

## Proposed behavior (sketch)

- In dependency tables (e.g. workspace or feature **conda-style** dependency sections), support a form such as:

  ```toml
  [dependencies]
  python = ">=3.11"
  pypi-txt = ["requirements.txt", "other-requirements.txt"]
  ```

- Pixi parses those files with the **same semantics as** `pixi import --format=pypi-txt` (PEP 508, `-r` recursion, etc.), and treats the result as PyPI requirements for solve/lock **without** copying them into the TOML.

- Clear errors (or documented limitations) for constructs that cannot be represented yet (e.g. unsupported constraint-file semantics), aligned with the [`pixi import` roadmap](https://github.com/prefix-dev/pixi/issues/4192).

## Why upstream might care

- **Adoption**: Lowers the barrier for teams that already standardize on `requirements.txt` or on conda envs that use `-r` under `pip:`.
- **Single source of truth**: Same idea as conda’s split between `dependencies:` and an external requirements file—one file for “what pip needs” until the team is ready to collapse into native Pixi tables.
- **Consistency**: Reuses the same parsing path as import, reducing behavioral surprises.

## Related issues

- [feat(import): `pixi import` roadmap #4192](https://github.com/prefix-dev/pixi/issues/4192) — PyPI `requirements.txt` import and `-r` goals.
- [bug(import): `pypi-txt` does not respect conditional platform dependencies #4598](https://github.com/prefix-dev/pixi/issues/4598) — environment markers / platform conditionals may need the same treatment for manifest-embedded `pypi-txt`.
- [Import conda-based environment with pip (`requirements.txt`) dependencies #1993](https://github.com/prefix-dev/pixi/issues/1993) — external `-r` from `environment.yml`.

## Possible discussion points for maintainers

- Whether `pypi-txt` belongs on **conda** dependency tables only, or also on **pypi** tables, and naming in `pixi.toml` vs `pyproject.toml`.
- Interaction with **`requires-pixi`** and lockfile stability when requirements files change.
- Documentation: phased migration guide and “when to fold into `[pypi-dependencies]`”.

