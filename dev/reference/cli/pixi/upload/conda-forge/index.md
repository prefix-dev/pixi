# `pixi upload conda-forge`

## About

Options for uploading to conda-forge

## Usage

```text
pixi upload conda-forge [OPTIONS] --staging-token <STAGING_TOKEN> --feedstock <FEEDSTOCK> --feedstock-token <FEEDSTOCK_TOKEN>
```

## Options

- [`--staging-token <STAGING_TOKEN>`](#arg---staging-token) : The Anaconda API key

  ```
  **required**: `true`
    
  **env**: `STAGING_BINSTAR_TOKEN`
  ```

- [`--feedstock <FEEDSTOCK>`](#arg---feedstock) : The feedstock name

  ```
  **required**: `true`
    
  **env**: `FEEDSTOCK_NAME`
  ```

- [`--feedstock-token <FEEDSTOCK_TOKEN>`](#arg---feedstock-token) : The feedstock token

  ```
  **required**: `true`
    
  **env**: `FEEDSTOCK_TOKEN`
  ```

- [`--staging-channel <STAGING_CHANNEL>`](#arg---staging-channel) : The staging channel name

  ```
  **env**: `STAGING_CHANNEL`
  ```

- [`--anaconda-url <ANACONDA_URL>`](#arg---anaconda-url) : The Anaconda Server URL

  ```
  **env**: `ANACONDA_SERVER_URL`
  ```

- [`--validation-endpoint <VALIDATION_ENDPOINT>`](#arg---validation-endpoint) : The validation endpoint url

  ```
  **env**: `VALIDATION_ENDPOINT`
  ```

- [`--provider <PROVIDER>`](#arg---provider) : The CI provider

  ```
  **env**: `CI`
  ```

- [`--dry-run`](#arg---dry-run) : Dry run, don't actually upload anything

  ```
  **env**: `DRY_RUN`
  ```
