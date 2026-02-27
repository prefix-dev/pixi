# Warnings Reference

This page lists the warning codes that can be encountered while using `pixi`. You can configure the behavior of these warnings in your `pixi.toml` file under the `[workspace.warnings]` section.

## Warning Codes

| Code | Short Code | Description |
| ---- | ---------- | ----------- |
| `task-input-missing` | `TI001` | A task input file specified in the manifest is missing on disk. |
| `project-deprecated` | `PD001` | A feature or field used in the project manifest is deprecated. |

## Configuration Example

To hide all task input warnings and fail on deprecated features, use the following configuration in your `pixi.toml`:

```toml
[workspace.warnings]
"TI*" = "hide"
"project-deprecated" = "fail"
```

For more details on how to configure warnings, see the [pixi.toml reference](./pixi_manifest.md#warnings).
