# [pixi](../../) [upload](../) cloudsmith

Options for uploading to a Cloudsmith repository. Authentication is used from the keychain / auth-file

## Usage

```text
pixi upload cloudsmith [OPTIONS] --owner <OWNER> --repo <REPO>
```

## Options

- [`--owner (-o) <OWNER>`](#arg---owner) : The owner (namespace) of the Cloudsmith repository

  ```
  **required**: `true`
    
  **env**: `CLOUDSMITH_OWNER`
  ```

- [`--repo (-r) <REPO>`](#arg---repo) : The Cloudsmith repository name

  ```
  **required**: `true`
    
  **env**: `CLOUDSMITH_REPO`
  ```

- [`--api-key (-a) <API_KEY>`](#arg---api-key) : The Cloudsmith API key, if none is provided, the token is read from the keychain / auth-file

  ```
  **env**: `CLOUDSMITH_API_KEY`
  ```

- [`--url (-u) <URL>`](#arg---url) : The URL to the Cloudsmith API server

  ```
  **env**: `CLOUDSMITH_API_URL`
  ```
