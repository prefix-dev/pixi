# shellcheck shell=bash
if [ -z "${_PIXI_WATCH_STAMP-}" ] || [ ! -f "$_PIXI_WATCH_STAMP" ]; then
    _PIXI_WATCH_STAMP=$(mktemp "${TMPDIR:-/tmp}/pixi_watch.XXXXXX" 2>/dev/null || echo "${TMPDIR:-/tmp}/pixi_watch_$$")
    touch "$_PIXI_WATCH_STAMP"
    trap 'rm -f "$_PIXI_WATCH_STAMP"' EXIT
fi

__pixi_watch_lock() {
    local lock_file="${PIXI_LOCK_FILE:-${PIXI_WORKSPACE_ROOT:-${PIXI_PROJECT_ROOT:-}}}"
    if [ -n "$lock_file" ] && [ ! -f "$lock_file" ]; then
        lock_file="${lock_file}/pixi.lock"
    fi
    if [ -n "$lock_file" ] && [ -f "$lock_file" ] && [ -n "${_PIXI_WATCH_STAMP-}" ] && [ -f "$_PIXI_WATCH_STAMP" ]; then
        if [ "$lock_file" -nt "$_PIXI_WATCH_STAMP" ]; then
            touch "$_PIXI_WATCH_STAMP"
            eval "$("$PIXI_EXE" shell-hook --change-ps1 false)"
            hash -r
        fi
    fi
}

pixi() {
    local first_arg="${1-}"

    "${PIXI_EXE-}" "$@" || return $?

    case "${first_arg-}" in
    add | a | remove | rm | install | i | update | u)
        if [ -n "${_PIXI_WATCH_STAMP-}" ] && [ -f "$_PIXI_WATCH_STAMP" ]; then
            touch "$_PIXI_WATCH_STAMP"
        fi
        eval "$("$PIXI_EXE" shell-hook --change-ps1 false)"
        hash -r
        ;;
    esac || :

    return 0
}

if [ -z "${PROMPT_COMMAND-}" ]; then
    PROMPT_COMMAND="__pixi_watch_lock"
else
    case ";$PROMPT_COMMAND;" in
    *";__pixi_watch_lock;"*) ;;
    *) PROMPT_COMMAND="__pixi_watch_lock;${PROMPT_COMMAND}" ;;
    esac
fi
