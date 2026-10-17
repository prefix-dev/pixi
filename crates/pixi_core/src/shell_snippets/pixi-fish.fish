if not set -q _PIXI_WATCH_STAMP; or not test -f "$_PIXI_WATCH_STAMP"
    set -gx _PIXI_WATCH_STAMP (mktemp (test -n "$TMPDIR"; and echo "$TMPDIR"; or echo "/tmp")/pixi_watch.XXXXXX 2>/dev/null; or echo "/tmp/pixi_watch_$fish_pid")
    touch "$_PIXI_WATCH_STAMP"
end

function __pixi_watch_lock --on-event fish_prompt
    set -l lock_file "$PIXI_LOCK_FILE"
    if test -z "$lock_file"
        if set -q PIXI_WORKSPACE_ROOT
            set lock_file "$PIXI_WORKSPACE_ROOT/pixi.lock"
        else if set -q PIXI_PROJECT_ROOT
            set lock_file "$PIXI_PROJECT_ROOT/pixi.lock"
        end
    else if not test -f "$lock_file"
        set lock_file "$lock_file/pixi.lock"
    end

    if test -n "$lock_file"; and test -f "$lock_file"; and set -q _PIXI_WATCH_STAMP; and test -f "$_PIXI_WATCH_STAMP"
        if test "$lock_file" -nt "$_PIXI_WATCH_STAMP"
            touch "$_PIXI_WATCH_STAMP"
            "$PIXI_EXE" shell-hook --change-ps1 false --shell fish | source
        end
    end
end

function pixi
    set -l first_arg $argv[1]
    command "$PIXI_EXE" $argv
    set -l exit_code $status
    if test $exit_code -ne 0
        return $exit_code
    end

    switch "$first_arg"
        case add a remove rm install i update u
            if set -q _PIXI_WATCH_STAMP; and test -f "$_PIXI_WATCH_STAMP"
                touch "$_PIXI_WATCH_STAMP"
            end
            "$PIXI_EXE" shell-hook --change-ps1 false --shell fish | source
    end
    return 0
end
