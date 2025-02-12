# shellcheck disable=all
pixi() {
    local first_arg="$1"
    local cmd="$PIXI_EXE $*"

    eval "$cmd"

    case "$first_arg" in
        add|a|remove|rm|install|i)
            eval "$($PIXI_EXE shell-hook --change-ps1 false)"
            rehash # Clear the command hash table in zsh
            ;;
    esac
}
