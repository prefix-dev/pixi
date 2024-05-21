/// Set default pixi prompt for the bash shell
pub fn get_bash_hook(env_name: &str) -> String {
    format!(
        "export PS1=\"({}) $PS1\"\n{}",
        env_name,
        include_str!("shell_snippets/pixi-bash.sh")
    )
}

/// Set default pixi prompt for the zsh shell
pub fn get_zsh_hook(env_name: &str) -> String {
    format!(
        "export PS1=\"({}) $PS1\"\n{}",
        env_name,
        include_str!("shell_snippets/pixi-zsh.sh")
    )
}

/// Set default pixi prompt for the fish shell
pub fn get_fish_prompt(env_name: &str) -> String {
    format!(
        r#"
        function __pixi_add_prompt
            set_color -o green
            echo -n "({}) "
            set_color normal
        end

        if not functions -q __fish_prompt_orig
            functions -c fish_prompt __fish_prompt_orig
        end

        if functions -q fish_right_prompt
            if not functions -q __fish_right_prompt_orig
                functions -c fish_right_prompt __fish_right_prompt_orig
            end
        else
            function __fish_right_prompt_orig
                # Placeholder function for when fish_right_prompt does not exist
                echo ""
            end
        end

        function fish_prompt
            set -l last_status $status
            if set -q PIXIE_LEFT_PROMPT
                __pixi_add_prompt
            end
            __fish_prompt_orig
            return $last_status
        end

        function fish_right_prompt
            if not set -q PIXIE_LEFT_PROMPT
                __pixi_add_prompt
            end
            __fish_right_prompt_orig
        end
        "#,
        env_name
    )
}

/// Set default pixi prompt for the xonsh shell
pub fn get_xonsh_prompt() -> String {
    // Xonsh' default prompt can find the environment for some reason.
    "".to_string()
}

/// Set default pixi prompt for the powershell
pub fn get_powershell_prompt(env_name: &str) -> String {
    format!(
        "$old_prompt = $function:prompt\n\
         function prompt {{\"({}) $($old_prompt.Invoke())\"}}",
        env_name
    )
}

/// Set default pixi prompt for the Nu shell
pub fn get_nu_prompt(env_name: &str) -> String {
    format!(
        "let old_prompt = $env.PROMPT_COMMAND; \
         $env.PROMPT_COMMAND = {{|| echo $\"\\({}\\) (do $old_prompt)\"}}",
        env_name
    )
}

/// Set default pixi prompt for the cmd.exe command prompt
pub fn get_cmd_prompt(env_name: &str) -> String {
    format!(r"@PROMPT ({}) $P$G", env_name)
}
