# JFrog Artifactory

JFrog Artifactory is an enterprise-grade artifact repository manager that supports conda packages.
This guide explains how to configure pixi to use Artifactory as a private conda channel.

## Setting up Artifactory

### 1. Create a conda repository

In your Artifactory instance, create a repository with the "Conda" package type.
The repository URL will be in the format: `https://my-org.jfrog.io/artifactory/<repository-name>/`

Artifactory supports different repository types:

- **Local repositories**: Store your own private packages
- **Remote repositories**: Cache and mirror upstream channels like conda-forge. This reduces external bandwidth, speeds up downloads, and provides availability even when upstream channels are down.
- **Virtual repositories** (not recommended): Combine multiple local and remote repositories under a single URL

A common setup is to create a remote repository that mirrors conda-forge, then combine it with a local repository for internal packages using a virtual repository.

![Artifactory repository overview](../assets/artifactory-repository-overview.png)

### 2. Generate an access token

To authenticate with Artifactory, you need to generate an access token:

1. Click on your user profile in the top-right corner and select **Set Me Up**

    ![Set Me Up menu](../assets/artifactory-set-me-up-menu.png)

2. Select **conda** as the package type

    ![Client type selection](../assets/artifactory-client-type-selection.png)

3. Select your conda repository

    ![Repository selection](../assets/artifactory-conda-client-repository.png)

4. Click **Generate Token & Create Instructions**

    ![Generate token button](../assets/artifactory-generate-token.png)

5. Copy the generated token

    ![Token generated](../assets/artifactory-token-generated.png)

## Authenticating with pixi

Use the `pixi auth login` command to authenticate with your Artifactory instance:

```shell
pixi auth login --token <artifactory-token> https://my-org.jfrog.io
```

This stores the token securely using your system's credential manager. See [Authentication](authentication.md) for more details on credential storage.

## Configuring channels

Add your Artifactory channel to your `pixi.toml`:

```toml
[workspace]
channels = ["https://my-org.jfrog.io/artifactory/channel-1", "conda-forge"]
```

!!!note "Strict channel priority"
    Pixi uses strict channel priority. Packages are always resolved from the first channel that contains them.
    In the example above, if a package exists in both your Artifactory channel and conda-forge,
    the version from Artifactory will always be used.

    This is useful for:

    - Overriding specific packages with internal builds
    - Ensuring consistent package versions across your organization
    - Using private packages that aren't available on public channels

    See [Channel Logic](../advanced/channel_logic.md) for more details on how channel priority works.

## Example configuration

Here's a complete example using Artifactory with conda-forge as a fallback:

```toml
[workspace]
name = "my-project"
channels = ["https://my-org.jfrog.io/artifactory/internal-packages", "conda-forge"]
platforms = ["linux-64", "osx-arm64", "win-64"]

[dependencies]
python = ">=3.11"
# This will come from your Artifactory channel if available there
my-internal-package = "*"
# These will come from conda-forge (due to channel priority)
numpy = ">=1.24"
pandas = ">=2.0"
```

### Forcing a specific channel

If you want to ensure a package always comes from a specific channel regardless of priority, use the `channel` key:

```toml
[dependencies]
# Always use numpy from conda-forge, even if it exists in Artifactory
numpy = { version = ">=1.24", channel = "conda-forge" }
# Always use internal-lib from Artifactory
internal-lib = { version = "*", channel = "https://my-org.jfrog.io/artifactory/internal-packages" }
```

This is useful when you want to override the default channel priority for specific packages.

## GitHub Actions with OIDC

For CI/CD pipelines, you can authenticate with Artifactory using OIDC (OpenID Connect) instead of storing long-lived tokens as secrets. This is more secure as tokens are short-lived and automatically rotated.

```yaml
- name: Log in to Artifactory
  uses: jfrog/setup-jfrog-cli@279b1f629f43dd5bc658d8361ac4802a7ef8d2d5 # v4.9.1
  id: artifactory
  env:
    JF_URL: https://my-org.jfrog.io
  with:
    disable-job-summary: true
    oidc-provider-name: ${{ vars.ARTIFACTORY_OIDC_PROVIDER }}
    oidc-audience: ${{ vars.ARTIFACTORY_OIDC_AUDIENCE }}

- name: Set up Pixi
  uses: prefix-dev/setup-pixi@82d477f15f3a381dbcc8adc1206ce643fe110fb7 # v0.9.3
  with:
    auth-host: https://my-org.jfrog.io
    auth-token: ${{ steps.artifactory.outputs.oidc-token }}
```

This requires configuring an OIDC provider in your Artifactory instance that trusts GitHub Actions. See JFrog's documentation on [OIDC integration](https://jfrog.com/help/r/jfrog-platform-administration-documentation/configure-an-oidc-integration) for setup instructions.
