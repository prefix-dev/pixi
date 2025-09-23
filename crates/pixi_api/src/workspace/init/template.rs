/// The pixi.toml template
///
/// This uses a template just to simplify the flexibility of emitting it.
pub const WORKSPACE_TEMPLATE: &str = r#"[workspace]
{%- if author %}
authors = ["{{ author[0] }} <{{ author[1] }}>"]
{%- endif %}
channels = {{ channels }}
name = "{{ name }}"
platforms = {{ platforms }}
version = "{{ version }}"

{%- if index_url or extra_index_urls %}

[pypi-options]
{% if index_url %}index-url = "{{ index_url }}"{% endif %}
{% if extra_index_urls %}extra-index-urls = {{ extra_index_urls }}{% endif %}
{%- endif %}

{%- if s3 %}
{%- for key in s3 %}

[workspace.s3-options.{{ key }}]
{%- if s3[key]["endpoint-url"] %}
endpoint-url = "{{ s3[key]["endpoint-url"] }}"
{%- endif %}
{%- if s3[key].region %}
{%- endif %}
{%- if s3[key].region %}
region = "{{ s3[key].region }}"
{%- endif %}
{%- if s3[key]["force-path-style"] is not none %}
force-path-style = {{ s3[key]["force-path-style"] }}
{%- endif %}

{%- endfor %}
{%- endif %}

[tasks]

[dependencies]

{%- if env_vars %}

[activation]
env = { {{ env_vars }} }
{%- endif %}

"#;

/// The pyproject.toml template
///
/// This is injected into an existing pyproject.toml
pub const PYROJECT_TEMPLATE_EXISTING: &str = r#"
[tool.pixi.workspace]
{%- if pixi_name %}
name = "{{ name }}"
{%- endif %}
channels = {{ channels }}
platforms = {{ platforms }}

[tool.pixi.pypi-dependencies]
{{ name }} = { path = ".", editable = true }
{%- for env, features in environments|items %}
{%- if loop.first %}

[tool.pixi.environments]
default = { solve-group = "default" }
{%- endif %}
{{env}} = { features = {{ features }}, solve-group = "default" }
{%- endfor %}

{%- if s3 %}
{%- for key in s3 %}

[tool.pixi.workspace.s3-options.{{ key }}]
{%- if s3[key]["endpoint-url"] %}
endpoint-url = "{{ s3[key]["endpoint-url"] }}"
{%- endif %}
{%- if s3[key].region %}
{%- endif %}
{%- if s3[key].region %}
region = "{{ s3[key].region }}"
{%- endif %}
{%- if s3[key]["force-path-style"] is not none %}
force-path-style = {{ s3[key]["force-path-style"] }}
{%- endif %}

{%- endfor %}
{%- endif %}

[tool.pixi.tasks]

"#;

/// The pyproject.toml template
///
/// This is used to create a pyproject.toml from scratch
pub const NEW_PYROJECT_TEMPLATE: &str = r#"[project]
{%- if author %}
authors = [{name = "{{ author[0] }}", email = "{{ author[1] }}"}]
{%- endif %}
dependencies = []
name = "{{ name }}"
requires-python = ">= 3.11"
version = "{{ version }}"

[build-system]
build-backend = "hatchling.build"
requires = ["hatchling"]

[tool.pixi.workspace]
channels = {{ channels }}
platforms = {{ platforms }}


{%- if index_url or extra_index_urls %}

[tool.pixi.pypi-options]
{% if index_url %}index-url = "{{ index_url }}"{% endif %}
{% if extra_index_urls %}extra-index-urls = {{ extra_index_urls }}{% endif %}
{%- endif %}

{%- if s3 %}
{%- for key in s3 %}

[tool.pixi.workspace.s3-options.{{ key }}]
{%- if s3[key]["endpoint-url"] %}
endpoint-url = "{{ s3[key]["endpoint-url"] }}"
{%- endif %}
{%- if s3[key].region %}
{%- endif %}
{%- if s3[key].region %}
region = "{{ s3[key].region }}"
{%- endif %}
{%- if s3[key]["force-path-style"] is not none %}
force-path-style = {{ s3[key]["force-path-style"] }}
{%- endif %}

{%- endfor %}
{%- endif %}

[tool.pixi.pypi-dependencies]
{{ pypi_package_name }} = { path = ".", editable = true }

[tool.pixi.tasks]

"#;

pub const GITIGNORE_TEMPLATE: &str = r#"
# pixi environments
.pixi/*
!.pixi/config.toml
"#;
