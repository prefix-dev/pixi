# shellcheck shell=bash
pixi() {
    local first_arg="$1"
    local cmd="$PIXI_EXE $*"

    eval "$cmd"
    local exit_code=$?

    if [ $exit_code -ne 0 ]; then
        return $exit_code
    fi

    case "$first_arg" in
        add|a|remove|rm|install|i)
            eval "$($PIXI_EXE shell-hook --change-ps1 false)"
            hash -r
            ;;
    esac
}
