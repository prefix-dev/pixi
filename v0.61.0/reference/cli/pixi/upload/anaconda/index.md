# `pixi upload anaconda`

## About

Options for uploading to a Anaconda.org server

## Usage

```text
pixi upload anaconda [OPTIONS] --owner <OWNER>
```

## Options

- [`--owner (-o) <OWNER>`](#arg---owner) : The owner of the distribution (e.g. conda-forge or your username)

  ```
  **required**: `true`
    
  **env**: `ANACONDA_OWNER`
  ```

- [`--channel (-c) <CHANNELS>`](#arg---channel) : The channel / label to upload the package to (e.g. main / rc)

  ```
  May be provided more than once.
    
  **env**: `ANACONDA_CHANNEL`
  ```

- [`--api-key (-a) <API_KEY>`](#arg---api-key) : The Anaconda API key, if none is provided, the token is read from the keychain / auth-file

  ```
  **env**: `ANACONDA_API_KEY`
  ```

- [`--url (-u) <URL>`](#arg---url) : The URL to the Anaconda server

  ```
  **env**: `ANACONDA_SERVER_URL`
  ```

- [`--force (-f)`](#arg---force) : Replace files on conflict

  ```
  **env**: `ANACONDA_FORCE`
  ```
