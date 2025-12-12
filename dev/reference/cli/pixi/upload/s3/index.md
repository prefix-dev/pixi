# [pixi](../../) [upload](../) s3

Options for uploading to S3

## Usage

```text
pixi upload s3 [OPTIONS] --channel <CHANNEL>
```

## Options

- [`--channel (-c) <CHANNEL>`](#arg---channel) : The channel URL in the S3 bucket to upload the package to, e.g., `s3://my-bucket/my-channel`

  ```
  **required**: `true`
    
  **env**: `S3_CHANNEL`
  ```

- [`--force`](#arg---force) : Replace files if it already exists

## S3 Credentials

- [`--endpoint-url <ENDPOINT_URL>`](#arg---endpoint-url) : The endpoint URL of the S3 backend

  ```
  **env**: `S3_ENDPOINT_URL`
  ```

- [`--region <REGION>`](#arg---region) : The region of the S3 backend

  ```
  **env**: `S3_REGION`
  ```

- [`--access-key-id <ACCESS_KEY_ID>`](#arg---access-key-id) : The access key ID for the S3 bucket

  ```
  **env**: `S3_ACCESS_KEY_ID`
  ```

- [`--secret-access-key <SECRET_ACCESS_KEY>`](#arg---secret-access-key) : The secret access key for the S3 bucket

  ```
  **env**: `S3_SECRET_ACCESS_KEY`
  ```

- [`--session-token <SESSION_TOKEN>`](#arg---session-token) : The session token for the S3 bucket

  ```
  **env**: `S3_SESSION_TOKEN`
  ```

- [`--addressing-style <ADDRESSING_STYLE>`](#arg---addressing-style) : How to address the bucket

  ```
  **env**: `S3_ADDRESSING_STYLE`
    
  **default**: `virtual-host`
    
  **options**: `virtual-host`, `path`
  ```
