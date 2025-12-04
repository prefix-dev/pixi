# `pixi upload prefix`

## About

Options for uploading to a prefix.dev server. Authentication is used from the keychain / auth-file

## Usage

```text
pixi upload prefix [OPTIONS] --channel <CHANNEL>
```

## Options

- [`--url (-u) <URL>`](#arg---url) : The URL to the prefix.dev server (only necessary for self-hosted instances)

  ```
  **env**: `PREFIX_SERVER_URL`
    
  **default**: `https://prefix.dev`
  ```

- [`--channel (-c) <CHANNEL>`](#arg---channel) : The channel to upload the package to

  ```
  **required**: `true`
    
  **env**: `PREFIX_CHANNEL`
  ```

- [`--api-key (-a) <API_KEY>`](#arg---api-key) : The prefix.dev API key, if none is provided, the token is read from the keychain / auth-file

  ```
  **env**: `PREFIX_API_KEY`
  ```

- [`--attestation <ATTESTATION>`](#arg---attestation) : Upload one or more attestation files alongside the package Note: if you add an attestation, you can *only* upload a single package

- [`--generate-attestation`](#arg---generate-attestation) : Automatically generate attestations when using trusted publishing

  ```
  **default**: `false`
  ```

- [`--skip-existing (-s)`](#arg---skip-existing) : Skip upload if package is existed
