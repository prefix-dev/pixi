#!/usr/bin/env bash
set -euo pipefail
# Version: v0.41.4

__wrap__() {

VERSION="${PIXI_VERSION:-latest}"
PIXI_HOME="${PIXI_HOME:-$HOME/.pixi}"
PIXI_HOME="${PIXI_HOME/#\~/$HOME}"
BIN_DIR="$PIXI_HOME/bin"

REPO="prefix-dev/pixi"
PLATFORM="$(uname -s)"
ARCH="${PIXI_ARCH:-$(uname -m)}"

if [[ $PLATFORM == "Darwin" ]]; then
  PLATFORM="apple-darwin"
elif [[ $PLATFORM == "Linux" ]]; then
  PLATFORM="unknown-linux-musl"
elif [[ $(uname -o) == "Msys" ]]; then
  PLATFORM="pc-windows-msvc"
fi

if [[ $ARCH == "arm64" ]] || [[ $ARCH == "aarch64" ]]; then
  ARCH="aarch64"
fi



BINARY="pixi-${ARCH}-${PLATFORM}"
EXTENSION="tar.gz"
if [[ $(uname -o) == "Msys" ]]; then
  EXTENSION="zip"
fi

if [[ $VERSION == "latest" ]]; then
  DOWNLOAD_URL="https://github.com/${REPO}/releases/latest/download/${BINARY}.${EXTENSION}"
else
  # Check if version is incorrectly specified without prefix 'v', and prepend 'v' in this case
  if [[ ! "$VERSION" =~ ^v ]]; then
    VERSION="v$VERSION"
  fi
  DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${VERSION}/${BINARY}.${EXTENSION}"
fi

printf "This script will automatically download and install Pixi (%s) for you.\nGetting it from this url: %s\n" "$VERSION" "$DOWNLOAD_URL"

if ! hash curl 2> /dev/null && ! hash wget 2> /dev/null; then
  echo "error: you need either 'curl' or 'wget' installed for this script."
  exit 1
fi

if ! hash tar 2> /dev/null; then
  echo "error: you do not have 'tar' installed which is required for this script."
  exit 1
fi

TEMP_FILE="$(mktemp "${TMPDIR:-/tmp}/.pixi_install.XXXXXXXX")"

cleanup() {
  rm -f "$TEMP_FILE"
}

trap cleanup EXIT

# Test if stdout is a terminal before showing progress
if [[ ! -t 1 ]]; then
  CURL_OPTIONS="--silent"  # --no-progress-meter is better, but only available in 7.67+
  WGET_OPTIONS="--no-verbose"
else
  CURL_OPTIONS="--no-silent"
  WGET_OPTIONS="--show-progress"
fi

if hash curl 2> /dev/null; then
  # Check that the curl version is not 8.8.0, which is broken for --write-out
  # https://github.com/curl/curl/issues/13845
  if [[ "$(curl --version | head -n 1 | cut -d ' ' -f 2)" == "8.8.0" ]]; then
    echo "error: curl 8.8.0 is known to be broken, please use a different version"
    if [[ $(uname -o) == "Msys" ]]; then
      echo "A common way to get an updated version of curl is to upgrade Git for Windows:"
      echo "      https://gitforwindows.org/"
    fi
    exit 1
  fi
  HTTP_CODE="$(curl -SL $CURL_OPTIONS "$DOWNLOAD_URL" --output "$TEMP_FILE" --write-out "%{http_code}")"
  if [[ "${HTTP_CODE}" -lt 200 || "${HTTP_CODE}" -gt 299 ]]; then
    echo "error: '${DOWNLOAD_URL}' is not available"
    exit 1
  fi
elif hash wget 2> /dev/null; then
  if ! wget $WGET_OPTIONS --output-document="$TEMP_FILE" "$DOWNLOAD_URL"; then
    echo "error: '${DOWNLOAD_URL}' is not available"
    exit 1
  fi
fi

# Check that file was correctly created (https://github.com/prefix-dev/pixi/issues/446)
if [[ ! -s "$TEMP_FILE" ]]; then
  echo "error: temporary file ${TEMP_FILE} not correctly created."
  echo "       As a workaround, you can try set TMPDIR env variable to directory with write permissions."
  exit 1
fi

# Extract pixi from the downloaded file
mkdir -p "$BIN_DIR"
if [[ "$(uname -o)" == "Msys" ]]; then
  unzip "$TEMP_FILE" -d "$BIN_DIR"
else
  # Extract to a temporary directory first
  TEMP_DIR=$(mktemp -d)
  tar -xzf "$TEMP_FILE" -C "$TEMP_DIR"

  # Find and move the `pixi` binary, making sure to handle the case where it's in a subdirectory
  if [[ -f "$TEMP_DIR/pixi" ]]; then
    mv "$TEMP_DIR/pixi" "$BIN_DIR/"
  else
    mv "$(find "$TEMP_DIR" -type f -name pixi)" "$BIN_DIR/"
  fi

  chmod +x "$BIN_DIR/pixi"
  rm -rf "$TEMP_DIR"
fi

echo "The 'pixi' binary is installed into '${BIN_DIR}'"

update_shell() {
    FILE="$1"
    LINE="$2"

    # shell update can be suppressed by `PIXI_NO_PATH_UPDATE` env var
    [[ -n "${PIXI_NO_PATH_UPDATE:-}" ]] && echo "No path update because PIXI_NO_PATH_UPDATE has a value" && return

    # Create the file if it doesn't exist
    if [ ! -f "$FILE" ]; then
        touch "$FILE"
    fi

    # Append the line if not already present
    if ! grep -Fxq "$LINE" "$FILE"
    then
        echo "Updating '${FILE}'"
        echo "$LINE" >> "$FILE"
        echo "Please restart or source your shell."
    fi
}

case "$(basename "$SHELL")" in
    bash)
        # Default to bashrc as that is used in non login shells instead of the profile.
        LINE="export PATH=\"${BIN_DIR}:\$PATH\""
        update_shell ~/.bashrc "$LINE"
        ;;

    fish)
        LINE="fish_add_path ${BIN_DIR}"
        update_shell ~/.config/fish/config.fish "$LINE"
        ;;

    zsh)
        LINE="export PATH=\"${BIN_DIR}:\$PATH\""
        update_shell ~/.zshrc "$LINE"
        ;;

    tcsh)
        LINE="set path = ( ${BIN_DIR} \$path )"
        update_shell ~/.tcshrc "$LINE"
        ;;

    *)
        echo "Could not update shell: $(basename "$SHELL")"
        echo "Please permanently add '${BIN_DIR}' to your \$PATH to enable the 'pixi' command."
        ;;
esac

}; __wrap__
