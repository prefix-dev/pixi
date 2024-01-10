# Proposal Design: Support for Private `pip` registries

## Objective

The aim is to support private `pip` package registries in pixi.

## Motivation

To deliver and deploy internal software, organizations often host packages in private registries requiring authentication. Examples of such solutions are:

* Google Cloud Artifact Registry
* AWS CodeArtifact
* Artifactory
* packagecloud
* ...etc.

To use such repositories with pixi, authentication should be supported.

## Design considerations

1. **Compatibility**. Different package servers use different ways of deriving credentials. Some use static usernames and passwords, some use temporary tokens that need to be regenerated once in a while. Ideally, `pixi` itself should not be concerned about those details.
2. **Ease of use**. In environments where authentication credentials can be derived automatically (e.g. GCP ADC), authentication should work "natively".
3. **Configurable**. In `pixi.toml` file, developers should be able to choose what authentication mechanism to choose for a given channel.
4. **Separation of concerns**. Authentication implementation for different platforms can be separate from `pixi` itself.

## Proposed solution

### Support for authentication helpers in `pixi`

Authentication helper is a program that receives an URL to the package registry as an input and outputs a JSON object containing the credentials. Authentication helpers can utilize different methods for obtaining credentials, but it's expected that they are specific to the package host implementation.

Authentication helper is a runnable binary available in `PATH`. The name of the binary must start with `pixi-auth-` and followed by its unique ID.

A number of authentication helpers is shipped together with `pixi`. However, the interface between `pixi` and authentication helpers is documented, and it's expected that users may add a helper specific to their usecase.

Authentication helpers are loosely inspired by `GIT_ASKPASS`, and authentication helpers in Docker and Flyte.

### `pixi` interface for authentication helpers

Every authentication helper program should, in case of success, return a JSON object with the following structure:

```
{
    "username": "<username>",
    "password": "<password>"
}
```

Upon success, authentication helpers exit with code `0`.

Upon failure, authentication helpers exit with code other than `0`. Authentication helpers must print a human-readable error message to stderr. `pixi` will relay the message back to the user.

Authentication helpers are never launched in interactive mode. Authentication helpers never receive user input over `stdin`, and can't print any additional information upon success.

### Example invocations of authentication helpers

1.
    ```
    pixi-auth-gcloud https://us-west1-python.pkg.dev/project-name/repo-name/simple
    ```

    Exit code: `0`

    `stdout`:

    ```
    {
        "username": "oauth2accesstoken",
        "password": "eWVhaCB0aGlzIGlzIGEgdG9rZW4gYWxyaWdodA=="
    }
    ```

1.
    ```
    pixi-auth-gcloud https://us-west1-python.pkg.dev/project-name/repo-name/simple
    ```

    Exit code: `1`

    `stdout`:

    ```
    Unable to derive authentication token: metadata server replied with: 500 Internal Server Error
    ```


### Referencing authentication helpers from `pixi.toml`

Users can define additional index URLs via `pypi-indices` block. Every additional repository can have an authentication helper defined for it.

```toml
[pypi-indices.artifact-registry]
url = "https://us-west1-python.pkg.dev/aaaaaaa/bbbbbbb/simple"
cred_helper = "gcloud"
```

`pip` packages can reference a different index.

```toml
[pypi-dependencies]
tensorflow = {version = "==2.14.0", index="artifact-registry"}
```

### Caching of auth credentials

`pixi` can cache credentials returned by helpers. If the remote repository returns HTTP codes `401` or `403` when a cached credential is used, `pixi` should try to refresh the credential and try accessing the registry again. Upon subsequent failure, no further attempts is performed, and the error is shown to the user.
