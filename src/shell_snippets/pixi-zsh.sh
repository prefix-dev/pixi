pixi() {
    local first_arg="$1"
    local cmd="$PIXI_EXE $*"

    eval "$cmd"

    case "$first_arg" in
        add|remove|install)
            eval "$($PIXI_EXE shell-hook)"
            rehash # Clear the command hash table in zsh
            ;;
    esac
}
