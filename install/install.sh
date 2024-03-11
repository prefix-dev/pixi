#!/usr/bin/env bash
set -euo pipefail

__wrap__() {

VERSION=${PIXI_VERSION:-latest}
PIXI_HOME=${PIXI_HOME:-"$HOME/.pixi"}
BIN_DIR="$PIXI_HOME/bin"

REPO=prefix-dev/pixi
PLATFORM=$(uname -s)
ARCH=$(uname -m)

if [[ $PLATFORM == "Darwin" ]]; then
  PLATFORM="apple-darwin"
elif [[ $PLATFORM == "Linux" ]]; then
  PLATFORM="unknown-linux-musl"
fi

if [[ $ARCH == "arm64" ]] || [[ $ARCH == "aarch64" ]]; then
  ARCH="aarch64"
fi



BINARY="pixi-${ARCH}-${PLATFORM}"

if [[ $VERSION == "latest" ]]; then
  DOWNLOAD_URL=https://github.com/${REPO}/releases/latest/download/${BINARY}.tar.gz
else
  DOWNLOAD_URL=https://github.com/${REPO}/releases/download/${VERSION}/${BINARY}.tar.gz
fi

printf "This script will automatically download and install Pixi (${VERSION}) for you.\nGetting it from this url: $DOWNLOAD_URL\n"

if ! hash curl 2> /dev/null && ! hash wget 2> /dev/null; then
  echo "error: you need either 'curl' or 'wget' installed for this script."
  exit 1
fi

if ! hash tar 2> /dev/null; then
  echo "error: you do not have 'tar' installed which is required for this script."
  exit 1
fi

TEMP_FILE=$(mktemp "${TMPDIR:-/tmp}/.pixi_install.XXXXXXXX")

cleanup() {
  rm -f "$TEMP_FILE"
}

trap cleanup EXIT

if hash curl 2> /dev/null; then
  HTTP_CODE=$(curl -SL --progress-bar "$DOWNLOAD_URL" --output "$TEMP_FILE" --write-out "%{http_code}")
  if [[ ${HTTP_CODE} -lt 200 || ${HTTP_CODE} -gt 299 ]]; then
    echo "error: '${DOWNLOAD_URL}' is not available"
    exit 1
  fi
elif hash wget 2> /dev/null; then
  if ! wget -q --show-progress --output-document="$TEMP_FILE" "$DOWNLOAD_URL"; then
    echo "error: '${DOWNLOAD_URL}' is not available"
    exit 1
  fi
fi

# Check that file was correctly created (https://github.com/prefix-dev/pixi/issues/446)
if [[ ! -s $TEMP_FILE ]]; then
  echo "error: temporary file ${TEMP_FILE} not correctly created."
  echo "       As a workaround, you can try set TMPDIR env variable to directory with write permissions."
  exit 1
fi

# Extract pixi from the downloaded tar file
mkdir -p "$BIN_DIR"
tar -xzf "$TEMP_FILE" -C "$BIN_DIR"
chmod +x "$BIN_DIR/pixi"
echo "The 'pixi' binary is installed into '${BIN_DIR}'"

update_shell() {
    FILE=$1
    LINE=$2

    # shell update can be suppressed by `PIXI_NO_PATH_UPDATE` env var
    [[ ! -z "${PIXI_NO_PATH_UPDATE-}" ]] && echo "No path update because PIXI_NO_PATH_UPDATE has a value" && return

    # Create the file if it doesn't exist
    if [ -f "$FILE" ]; then
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
        if [ -f ~/.bash_profile ]; then
            BASH_FILE=~/.bash_profile
        else
            # Default to bashrc as that is used in non login shells instead of the profile.
            BASH_FILE=~/.bashrc
        fi
        LINE="export PATH=\$PATH:${BIN_DIR}"
        update_shell $BASH_FILE "$LINE"
        ;;

    fish)
        LINE="fish_add_path ${BIN_DIR}"
        update_shell ~/.config/fish/config.fish "$LINE"
        ;;

    zsh)
        LINE="export PATH=\$PATH:${BIN_DIR}"
        update_shell ~/.zshrc "$LINE"
        ;;

    tcsh)
        LINE="set path = ( \$path ${BIN_DIR} )"
        update_shell ~/.tcshrc "$LINE"
        ;;

    *)
        echo "Could not update shell: $(basename "$SHELL")"
        echo "Please permanently add '${BIN_DIR}' to your \$PATH to enable the 'pixi' command."
        ;;
esac

}; __wrap__
