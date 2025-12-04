# `pixi upload artifactory`

## About

Options for uploading to a Artifactory channel. Authentication is used from the keychain / auth-file

## Usage

```text
pixi upload artifactory [OPTIONS] --url <URL> --channel <CHANNELS>
```

## Options

- [`--url (-u) <URL>`](#arg---url) : The URL to your Artifactory server

  ```
  **required**: `true`
    
  **env**: `ARTIFACTORY_SERVER_URL`
  ```

- [`--channel (-c) <CHANNELS>`](#arg---channel) : The URL to your channel

  ```
  **required**: `true`
    
  **env**: `ARTIFACTORY_CHANNEL`
  ```

- [`--token (-t) <TOKEN>`](#arg---token) : Your Artifactory token

  ```
  **env**: `ARTIFACTORY_TOKEN`
  ```
