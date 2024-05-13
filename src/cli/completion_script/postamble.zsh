_pixi_run_inner() {
    if [[ $words[2] == "run" ]]; then
        local tasks
        tasks=("${(@s/ /)$(pixi task list --summary 2> /dev/null)}")
        tasks=("${(@)tasks:#}")  # Remove empty elements

        if [[ $CURRENT -eq 3 && -n "$tasks" ]]; then
            _values 'task' "${tasks[@]}"
        else
            # Delegate to the completion system of the command after 'pixi run'
            shift 2 words
            (( CURRENT -= 2 ))
            _normal
        fi
    else
        _pixi
    fi
}

# Load the completion function
compdef _pixi_run_inner pixi
