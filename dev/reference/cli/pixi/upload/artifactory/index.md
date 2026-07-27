# [pixi](../../) [upload](../) artifactory

Options for uploading to an Artifactory channel

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

- [`--username <USERNAME>`](#arg---username) : Your Artifactory username for HTTP basic authentication

  ```
  **env**: `ARTIFACTORY_USERNAME`
  ```

- [`--password <PASSWORD>`](#arg---password) : Your Artifactory password for HTTP basic authentication

  ```
  **env**: `ARTIFACTORY_PASSWORD`
  ```

- [`--token (-t) <TOKEN>`](#arg---token) : Your Artifactory token for bearer authentication

  ```
  **env**: `ARTIFACTORY_TOKEN`
  ```

## Description

Options for uploading to an Artifactory channel.

Authentication can be supplied directly with a bearer token or with an Artifactory username and password. If no credentials are supplied, they are read from the keychain / auth-file.
