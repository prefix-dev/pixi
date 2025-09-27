use pixi_manifest::EnvironmentName;
use rattler_shell::shell::ShellEnum;

/// Sets default pixi hook for the bash shell
pub fn bash_hook() -> &'static str {
    include_str!("shell_snippets/pixi-bash.sh")
}

/// Sets default pixi hook for the zsh shell
pub fn zsh_hook() -> &'static str {
    include_str!("shell_snippets/pixi-zsh.sh")
}

/// Sets default pixi prompt for posix shells
pub fn posix_prompt(env_name: &str) -> String {
    format!("export PS1=\"({}) ${{PS1:-}}\"", env_name)
}

/// Sets default pixi prompt for the fish shell
pub fn fish_prompt(env_name: &str) -> String {
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
            if set -q PIXI_LEFT_PROMPT
                __pixi_add_prompt
            end
            __fish_prompt_orig
            return $last_status
        end

        function fish_right_prompt
            if not set -q PIXI_LEFT_PROMPT
                __pixi_add_prompt
            end
            __fish_right_prompt_orig
        end
        "#,
        env_name
    )
}

/// Sets default pixi prompt for the xonsh shell
pub fn xonsh_prompt() -> String {
    // Xonsh' default prompt can find the environment for some reason.
    "".to_string()
}

/// Sets default pixi prompt for the powershell
pub fn powershell_prompt(env_name: &str) -> String {
    format!(
        "$old_prompt = $function:prompt\n\
         function prompt {{\"({}) $($old_prompt.Invoke())\"}}",
        env_name
    )
}

/// Sets default pixi prompt for the Nu shell
pub fn nu_prompt(env_name: &str) -> String {
    format!(
        "let old_prompt = $env.PROMPT_COMMAND; \
         $env.PROMPT_COMMAND = {{|| echo $\"\\({}\\) (do $old_prompt)\"}}",
        env_name
    )
}

/// Sets default pixi prompt for the cmd.exe command prompt
pub fn cmd_prompt(env_name: &str) -> String {
    format!(r"@PROMPT ({}) $P$G", env_name)
}

/// Returns appropriate hook function for configured shell
pub fn shell_hook(shell: &ShellEnum) -> Option<&str> {
    match shell {
        ShellEnum::Bash(_) => Some(bash_hook()),
        ShellEnum::Zsh(_) => Some(zsh_hook()),
        _ => None,
    }
}

/// Returns appropriate prompt (without hook) for configured shell
pub fn shell_prompt(shell: &ShellEnum, prompt_name: &str) -> String {
    match shell {
        ShellEnum::NuShell(_) => nu_prompt(prompt_name),
        ShellEnum::PowerShell(_) => powershell_prompt(prompt_name),
        ShellEnum::Bash(_) => posix_prompt(prompt_name),
        ShellEnum::Zsh(_) => posix_prompt(prompt_name),
        ShellEnum::Fish(_) => fish_prompt(prompt_name),
        ShellEnum::Xonsh(_) => xonsh_prompt(),
        ShellEnum::CmdExe(_) => cmd_prompt(prompt_name),
    }
}

/// Returns prompt name for given project and environment
pub fn prompt_name(project_name: &str, environment_name: &EnvironmentName) -> String {
    match environment_name {
        EnvironmentName::Default => project_name.to_string(),
        EnvironmentName::Named(name) => format!("{}:{}", project_name, name),
    }
}
