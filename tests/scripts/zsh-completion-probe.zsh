#!/usr/bin/env zsh
#
# Report what the generated zsh completion inserts for one command line.
#
#   zsh-completion-probe.zsh <completion-dir> <workspace-dir> <line>
#
# Loads `<completion-dir>/_pixi` into a real interactive zsh, types <line>,
# presses Tab, and prints the resulting command line on stdout. Driving an
# actual shell is the only way to cover how `_arguments` invokes our action:
# it prepends its own `compadd` options, which a direct call to the helper
# would never show.
#
# The command line is read back through a widget that writes `$BUFFER` to a
# file rather than by scraping the terminal, so the result is exact.
#
# Every wait below gives up after $TIMEOUT seconds. A probe shell that never
# prompts, never finishes setup or never reports a buffer has to fail the
# caller rather than hang it.

emulate -L zsh
setopt no_unset

zmodload zsh/zpty
zmodload zsh/datetime

# Seconds to wait for each step, and how often to look while waiting. Plain
# scalars, not `typeset -F`, so they read as `5` rather than `5.000000000` in
# the messages below and in `sleep`.
typeset TIMEOUT=5
typeset POLL=0.05

completion_dir=${1:?completion dir}
workspace=${2:?workspace dir}
line=${3:?line to complete}

buffer_file=$workspace/.probe-buffer
rm -f -- $buffer_file

typeset prompt='PROBE-READY>'
typeset chunk

# `-f` keeps the user's startup files out of it; `-i` is needed for zle, which
# is what makes Tab run completion at all.
zpty -b probe "PS1='$prompt' zsh -f -i"
trap 'zpty -d probe 2>/dev/null' EXIT

# Wait until the shell prompts, so setup cannot race its startup.
#
# Polls and accumulates instead of letting `zpty -r` block on the pattern
# itself: a blocking read has no timeout, and whether `-t` keeps unmatched
# output for the next call is not something to depend on.
await_prompt() {
    local seen=
    local -F deadline=$(( EPOCHREALTIME + TIMEOUT ))
    while (( EPOCHREALTIME < deadline )); do
        while zpty -r -t probe chunk 2>/dev/null; do
            seen+=$chunk
            [[ $seen == *${prompt}* ]] && return 0
        done
        sleep $POLL
    done
    return 1
}

if ! await_prompt; then
    print -u2 "zsh-completion-probe: no prompt from the probe shell within ${TIMEOUT}s"
    exit 1
fi

zpty -w probe "fpath=(${(q)completion_dir} \$fpath)"
zpty -w probe "autoload -Uz compinit && compinit -u -d ${(q)workspace}/.zcompdump"
# Tab must insert a unique match outright instead of opening a menu, so that
# the buffer alone tells us what was offered.
zpty -w probe 'unsetopt auto_menu auto_list'
zpty -w probe "report_buffer() { print -r -- \$BUFFER >! ${(q)buffer_file} }"
zpty -w probe 'zle -N report_buffer; bindkey "^G" report_buffer'
zpty -w probe "cd ${(q)workspace}"
if ! await_prompt; then
    print -u2 "zsh-completion-probe: the probe shell did not finish setup within ${TIMEOUT}s"
    exit 1
fi

# `-n` sends the keys without a newline, so nothing is executed.
zpty -w -n probe "$line"
zpty -w -n probe $'\t'
zpty -w -n probe $'\C-g'

typeset -F deadline=$(( EPOCHREALTIME + TIMEOUT ))
while (( EPOCHREALTIME < deadline )); do
    [[ -s $buffer_file ]] && break
    # Keep draining, or the pty fills up and the shell stops making progress.
    zpty -r -t probe chunk >/dev/null 2>&1
    sleep $POLL
done

if [[ ! -s $buffer_file ]]; then
    print -u2 "zsh-completion-probe: the probe shell reported no buffer within ${TIMEOUT}s"
    exit 1
fi

cat -- $buffer_file
