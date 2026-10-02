# Audit locked dependencies

`pixi audit` queries the public [Basilisk](https://api.basilisk.prefix.dev) vulnerability API for packages in your lock file.

```shell
pixi audit
pixi audit --frozen --json
pixi audit --environment default --platform linux-64
```

The command exits with status `1` when it finds non-ignored vulnerabilities. Packages outside the supported ecosystems are reported as **not checked**, not as safe. Auditing requires network access; offline mode does not produce an up-to-date vulnerability report.

## Optional sign-in

Auditing does **not** require authentication. To explicitly obtain a Basilisk-specific prefix.dev token before auditing:

```shell
pixi audit --login
```

This uses the existing OAuth login flow. The audience credential is stored separately from package-channel credentials and reused or refreshed on subsequent audits. Normal `pixi audit` never starts an interactive login. Missing credentials, login failures, and refresh failures do not prevent a public audit.

`--login` is skipped in CI, noninteractive, or offline mode. The usual authentication storage configuration applies, including `RATTLER_AUTH_FILE` and `--auth-file`. Tokens are not included in the audit report.

Audit API requests use a dedicated client, without package mirrors or channel authentication. Proxy and TLS configuration are preserved, redirects are disabled, and audience credentials are not used when TLS verification is disabled.

## Ignore a finding

Use an advisory ID or alias:

```toml
[workspace.audit]
ignore = ["CVE-2026-0001", "GHSA-xxxx-yyyy-zzzz"]
```

## Custom API

Set `PIXI_AUDIT_BASE_URL` to use a compatible API, for example a self-hosted instance. URLs must use HTTP(S) and must not contain credentials, query parameters, or fragments. Include a trailing slash when using a path prefix.

Custom origins are queried anonymously: Pixi never forwards the production Basilisk audience token to them, and `--login` does not initiate a login for a custom origin.
