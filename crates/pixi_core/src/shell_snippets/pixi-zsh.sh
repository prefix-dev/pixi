# shellcheck disable=all
if [[ -z "${_PIXI_WATCH_STAMP-}" || ! -f "$_PIXI_WATCH_STAMP" ]]; then
    _PIXI_WATCH_STAMP=$(mktemp "${TMPDIR:-/tmp}/pixi_watch.XXXXXX" 2>/dev/null || echo "${TMPDIR:-/tmp}/pixi_watch_$$")
    touch "$_PIXI_WATCH_STAMP"
    trap 'rm -f "$_PIXI_WATCH_STAMP"' EXIT
fi

__pixi_watch_lock() {
    local lock_file="${PIXI_LOCK_FILE:-${PIXI_WORKSPACE_ROOT:-${PIXI_PROJECT_ROOT:-}}}"
    if [[ -n "$lock_file" && ! -f "$lock_file" ]]; then
        lock_file="${lock_file}/pixi.lock"
    fi
    if [[ -n "$lock_file" && -f "$lock_file" && -n "${_PIXI_WATCH_STAMP-}" && -f "$_PIXI_WATCH_STAMP" ]]; then
        if [[ "$lock_file" -nt "$_PIXI_WATCH_STAMP" ]]; then
            touch "$_PIXI_WATCH_STAMP"
            eval "$("$PIXI_EXE" shell-hook --change-ps1 false)"
            rehash
        fi
    fi
}

pixi() {
    local first_arg="${1-}"

    "${PIXI_EXE-}" "$@" || return $?

    case "${first_arg-}" in
    add | a | remove | rm | install | i | update | u)
        if [[ -n "${_PIXI_WATCH_STAMP-}" && -f "$_PIXI_WATCH_STAMP" ]]; then
            touch "$_PIXI_WATCH_STAMP"
        fi
        eval "$("$PIXI_EXE" shell-hook --change-ps1 false)"
        rehash # Clear the command hash table in zsh
        ;;
    esac || :

    return 0
}

autoload -Uz add-zsh-hook 2>/dev/null && add-zsh-hook precmd __pixi_watch_lock || {
    if [[ -z "${precmd_functions[(r)__pixi_watch_lock]-}" ]]; then
        precmd_functions+=(__pixi_watch_lock)
    fi
}
