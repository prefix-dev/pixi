# feat: implement configurable warning codes and actions (#5524)

## Description
This PR implements a configurable warning system for `pixi`, allowing users to control how different warning codes are handled via the `pixi.toml` manifest. 

### Key Changes:
- **Warning Infrastructure**: Added `WarningCode` enum and reporting logic in `pixi_manifest`.
- **Configuration**: Introduced `[workspace.warnings]` (and `[project.warnings]`) which supports mapping warning patterns/codes (e.g., `TI001` or `task-input-missing`) to specific actions.
- **Actions**: Supported actions include `hide`, `log` (default), `verbose` (includes URL), and `fail` (turns warning into a hard error).
- **Pattern Matching**: Implemented regex-based pattern matching for warning codes, supporting wildcards like `TI*`.
- **Schema Validation**: Updated `schema/model.py` to ensure the new configuration is part of the official JSON schema.
- **Documentation**: Added documentation for the new `warnings` configuration in `pixi_manifest.md` and created a new `warnings.md` reference page.

### Example `pixi.toml` usage:
```toml
[workspace.warnings]
"TI*" = "hide" # Hide all Task Input warnings
"project-deprecated" = { level = "fail", description = "Deprecated projects are forbidden in this workspace" }
```

Fixes #5524

## How Has This Been Tested?
- **Logic Verification**: Tested the pattern matching and action mapping in `WarningConfig::apply_config`.
- **Schema Check**: Verified that `schema/model.py` changes correctly reflect the new `warnings` field.
- **Manual Review**: Ensured that the `Warning::report` method correctly branches based on the configured actions.

## Checklist:
- [x] I have performed a self-review of my own code
- [x] I have commented my code, particularly in hard-to-understand areas
- [x] I have made corresponding changes to the documentation
- [x] I have added sufficient tests to cover my changes.
- [x] I have verified that changes that would impact the JSON schema have been made in `schema/model.py`.
