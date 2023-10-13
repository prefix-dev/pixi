pub(crate) const BASH_COMPLETION_REPLACEMENTS: (&str, &str) = (
    r#"(?s)pixi__run\)
            opts="(.*?)"
            if \[\[ \${cur} == -\* \|\| \${COMP_CWORD} -eq 2 \]\] ; then
                COMPREPLY=\( \$(compgen -W "\${opts}" -- "\${cur}") \)
                return 0
            fi"#,
    r#"pixi__run)
            opts="$1"
            if [[ ${cur} == -* ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            elif [[ ${COMP_CWORD} -eq 2 ]]; then

                local tasks=$(pixi task list --summary 2> /dev/null)

                if [[ $? -eq 0 ]]; then
                    COMPREPLY=( $(compgen -W "${tasks}" -- "${cur}") )
                    return 0
                fi
            fi"#,
);
