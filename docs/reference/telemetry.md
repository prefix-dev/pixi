# Telemetry

Pixi sends a small, anonymous ping in two situations:

1. **After a successful installation** through the `install.sh` / `install.ps1` scripts.
2. **After `pixi self-update`** successfully updates the binary.

The install scripts run the freshly installed binary to send the ping, so both
pings come from Pixi itself. The installer prints a notice when it sends the
ping. `pixi self-update` only pings once you have seen that notice: if you have
not (for example because you installed Pixi another way), the first update
shows the notice instead of pinging, so you can opt out before anything is sent.
Pixi records that the notice was shown in a `telemetry-notice-shown` file in the
[cache directory](environment_variables.md); if that file is removed (for
example by `pixi clean cache`), the notice is shown again on the next update.

That is the *only* telemetry in Pixi. During normal use — resolving, installing,
building, running tasks — Pixi does not phone home. The ping exists so we can
estimate how many people install and update Pixi and on which platforms.

Both pings are **best-effort**: they use a short (3 second) timeout and any
error is ignored, so they can never block, slow down, or fail an install or
update.

## What is sent

Each ping encodes the following into the request:

- The **event** (`install` or `self-update`).
- The Pixi **version**.
- The **operating system** (`linux`, `macos`, or `windows`).
- The **CPU architecture** (e.g. `x86_64`, `aarch64`).

As with *any* HTTP request, the receiving server also sees standard request
metadata that Pixi does not add on purpose but cannot avoid:

- Your **IP address**.
- The **User-Agent**, which is `pixi/<version>`.
- The **time** of the request.

Pixi does **not** send your account, project contents, environment or package
lists, file paths, or any generated persistent/unique identifier. We do not
correlate pings into a per-machine profile.

!!! note
    We intentionally avoid claiming this is "completely anonymous" or contains
    "no personal data". Because the request carries your IP address, it is more
    accurate to describe it as **anonymous by design**: we only look at
    aggregate numbers, and nothing in the payload identifies you.

## Where it is sent

The ping goes to `https://installation-ping.prefix.dev`. Using our own domain
means we can point it at a different backend without updating the install
scripts or released Pixi binaries.

Behind that endpoint the request is handled by [Scarf](https://about.scarf.sh),
which aggregates the pings into install/update counts. Scarf receives the
request (including the metadata above) and processes it according to their
[privacy policy](https://about.scarf.sh/privacy-policy). We only use the
resulting aggregate statistics; we do not retain a raw per-request log for our
own analysis.

## How to opt out

Set either environment variable to a non-empty value before installing or updating. Pixi treats
`PIXI_NO_TELEMETRY` as its own convention and also honors the ecosystem-wide
[`DO_NOT_TRACK`](https://consoledonottrack.com) convention. Both disable the
ping in the install scripts **and** in `pixi self-update`.

=== "Linux & macOS"
    ```bash
    # During installation
    curl -fsSL https://pixi.sh/install.sh | PIXI_NO_TELEMETRY=1 bash

    # For self-update (and any future CLI ping), export it in your shell
    export PIXI_NO_TELEMETRY=1
    pixi self-update
    ```

=== "Windows"
    ```powershell
    # During installation
    $env:PIXI_NO_TELEMETRY=1; powershell -ExecutionPolicy Bypass -c "irm -useb https://pixi.sh/install.ps1 | iex"

    # For self-update (and any future CLI ping)
    $env:PIXI_NO_TELEMETRY=1
    pixi self-update
    ```

To disable it permanently, set the variable in your shell's startup file (e.g.
`~/.bashrc`, `~/.zshrc`, or your PowerShell profile).
