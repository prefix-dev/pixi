# `pixi upload quetz`

## About

Upload to a Quetz server. Authentication is used from the keychain / auth-file

## Usage

```text
pixi upload quetz [OPTIONS] --url <URL> --channel <CHANNELS>
```

## Options

- [`--url (-u) <URL>`](#arg---url) : The URL to your Quetz server

  ```
  **required**: `true`
    
  **env**: `QUETZ_SERVER_URL`
  ```

- [`--channel (-c) <CHANNELS>`](#arg---channel) : The URL to your channel

  ```
  **required**: `true`
    
  **env**: `QUETZ_CHANNEL`
  ```

- [`--api-key (-a) <API_KEY>`](#arg---api-key) : The Quetz API key, if none is provided, the token is read from the keychain / auth-file

  ```
  **env**: `QUETZ_API_KEY`
  ```
