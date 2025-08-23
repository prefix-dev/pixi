#!/bin/sh
# Don't exit due to unset vars
set -e
# Version: v0.53.0
#
readonly VERSION="${PIXI_VERSION:-latest}"
readonly REPOURL="${PIXI_REPOURL:-https://github.com/prefix-dev/pixi}"


__check__(){
    local bin="$1"
    command -v "${bin}" &> /dev/null|| return 1

}
__info__(){
    echo -e "[$(date "+%Y-%m-%d %H:%M:%S")][INFO]:["${1}"]"
}

__warn__(){
    echo -e "[$(date "+%Y-%m-%d %H:%M:%S")][WARN]:["${1}"]" >&2
}

__error__(){
    echo -e "[$(date "+%Y-%m-%d %H:%M:%S")][ERROR]:["${1}"]" >&2
}

__main__() {
    PIXI_HOME="${PIXI_HOME:-$HOME/.pixi}"
    case "$PIXI_HOME" in
    '~' | '~'/*) PIXI_HOME="${HOME-}${PIXI_HOME#\~}" ;; # expand tilde
    esac
    BIN_DIR="$PIXI_HOME/bin"


    PLATFORM="$(uname -s)"
    ARCH="${PIXI_ARCH:-$(uname -m)}"
    IS_MSYS=false

    if [ "${PLATFORM-}" = "Darwin" ]; then
        PLATFORM="apple-darwin"
    elif [ "${PLATFORM-}" = "Linux" ]; then
        PLATFORM="unknown-linux-musl"
    elif [ "$(uname -o)" = "Msys" ]; then
        IS_MSYS=true
        PLATFORM="pc-windows-msvc"
    fi

    case "${ARCH-}" in
    arm64 | aarch64) ARCH="aarch64" ;;
    esac

    BINARY="pixi-${ARCH}-${PLATFORM}"
    if $IS_MSYS; then
        EXTENSION=".zip"
        __check__ unzip || EXTENSION=".exe"
    else
        EXTENSION=".tar.gz"
        __check__ tar || EXTENSION=''
    fi

    if [ "$VERSION" = "latest" ]; then
        DOWNLOAD_URL="${REPOURL%/}/releases/latest/download/${BINARY}${EXTENSION-}"
    else
        # Check if version is incorrectly specified without prefix 'v', and prepend 'v' in this case
        DOWNLOAD_URL="${REPOURL%/}/releases/download/v${VERSION#v}/${BINARY}${EXTENSION-}"
    fi

    __info__ "This script will automatically download and install Pixi ($VERSION) for you.\nGetting it from this url: $DOWNLOAD_URL"

    HAVE_CURL=false
    HAVE_CURL_8_8_0=false
    if __check__ "curl" ; then
        # Check that the curl version is not 8.8.0, which is broken for --write-out
        # https://github.com/curl/curl/issues/13845
        if [ "$(curl --version | (
            IFS=' ' read -r _ v _
            printf %s "${v-}"
        ))" = "8.8.0" ]; then
            HAVE_CURL_8_8_0=true
        else
            HAVE_CURL=true
        fi
    fi

    HAVE_WGET=true
    __check__ wget || HAVE_WGET=false

    if ! $HAVE_CURL && ! $HAVE_WGET; then
        __error__ "you need either 'curl' or 'wget' installed for this script."
        if $HAVE_CURL_8_8_0; then
            __error__ "curl 8.8.0 is known to be broken, please use a different version"
            if $IS_MSYS; then
                __info__ "A common way to get an updated version of curl is to upgrade Git for Windows:"
                __info__ "https://gitforwindows.org/"
            fi
        fi
        exit 1
    fi

    TEMP_FILE="$(mktemp "${TMPDIR:-/tmp}/.pixi_install.XXXXXXXX")"

    cleanup() {
        rm -f "$TEMP_FILE"
    }

    trap cleanup EXIT

    # Test if stdout is a terminal before showing progress
    CURL_OPTIONS="--no-silent"
    WGET_OPTIONS="--show-progress"
    if [ ! -t 1 ]; then
        CURL_OPTIONS="--silent" # --no-progress-meter is better, but only available in 7.67+
        WGET_OPTIONS="--no-verbose"
    fi

    if $HAVE_CURL; then
        CURL_ERR=0
        HTTP_CODE="$(curl -SL $CURL_OPTIONS "$DOWNLOAD_URL" --output "$TEMP_FILE" --write-out "%{http_code}")" || CURL_ERR=$?
        case "$CURL_ERR" in
        35 | 53 | 54 | 59 | 66 | 77)
            if ! $HAVE_WGET; then
                __error__ "when download '${DOWNLOAD_URL}', curl has some local ssl problems with error $CURL_ERR" && exit 1

            fi
            # fallback to wget
            ;;
        0)
            if [ "${HTTP_CODE}" -lt 200 ] || [ "${HTTP_CODE}" -gt 299 ]; then
                __error__ " '${DOWNLOAD_URL}' is not available"&& exit 1
            fi
            HAVE_WGET=false # download success, skip wget
            ;;
        *)
            __error__ "when download '${DOWNLOAD_URL}', curl fails with with error $CURL_ERR" && exit 1
            ;;
        esac
    fi

    if $HAVE_WGET && ! wget $WGET_OPTIONS --output-document="$TEMP_FILE" "$DOWNLOAD_URL"; then
        __error__ "error: '${DOWNLOAD_URL}' is not available"&& exit 1
    fi

    # Check that file was correctly created (https://github.com/prefix-dev/pixi/issues/446)
    if [ ! -s "$TEMP_FILE" ]; then
        __error__ "temporary file ${TEMP_FILE} not correctly created."
        __info__ "As a workaround, you can try set TMPDIR env variable to directory with write permissions."
        exit 1
    fi

    # Extract pixi from the downloaded file
    mkdir -p "$BIN_DIR"
    if [ "${EXTENSION-}" = ".zip" ]; then
        unzip "$TEMP_FILE" -d "$BIN_DIR"
    elif [ "${EXTENSION-}" = ".tar.gz" ]; then
        # Extract to a temporary directory first
        TEMP_DIR=$(mktemp -d)
        tar -xzf "$TEMP_FILE" -C "$TEMP_DIR"

        # Find and move the `pixi` binary, making sure to handle the case where it's in a subdirectory
        if [ -f "$TEMP_DIR/pixi" ]; then
            mv "$TEMP_DIR/pixi" "$BIN_DIR/"
        else
            mv "$(find "$TEMP_DIR" -type f -name pixi)" "$BIN_DIR/"
        fi

        chmod +x "$BIN_DIR/pixi"
        rm -rf "$TEMP_DIR"
    elif [ "${EXTENSION-}" = ".exe" ]; then
        cp -f "$TEMP_FILE" "$BIN_DIR/pixi.exe"
    else
        chmod +x "$TEMP_FILE"
        cp -f "$TEMP_FILE" "$BIN_DIR/pixi"
    fi

    __info__ "The 'pixi' binary is installed into '${BIN_DIR}'"

    # shell update can be suppressed by `PIXI_NO_PATH_UPDATE` env var
    if [ -n "${PIXI_NO_PATH_UPDATE:-}" ]; then
        __warn__ "No path update because PIXI_NO_PATH_UPDATE is set"
    else
        update_shell() {
            FILE="$1"
            LINE="$2"

            # Create the file if it doesn't exist
            if [ ! -f "$FILE" ]; then
                touch "$FILE"
            fi

            # Append the line if not already present
            if ! grep -Fxq "$LINE" "$FILE"; then
                __info__ "Updating '${FILE}'"
                echo "$LINE" >>"$FILE"
                __info__ "Please restart or source your shell."
            fi
        }

        case "$(basename "${SHELL-}")" in
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

        '')
            __warn__ "Could not detect shell type."
            __info__  "Please permanently add '${BIN_DIR}' to your \$PATH to enable the 'pixi' command."
            ;;

        *)
                __warn__ "Could not update shell $(basename "$SHELL")"
                __info__ "Please permanently add '${BIN_DIR}' to your \$PATH to enable the 'pixi' command."
            ;;
        esac
    fi
}

{
   ## Load all script before execution
   __main__

}
