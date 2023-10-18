/// Set default pixi prompt for the bash shell
#[cfg(target_family = "unix")]
pub fn get_bash_prompt(env_name: &str) -> String {
    format!("export PS1=\"({}) $PS1\"", env_name)
}

/// Set default pixi prompt for the zsh shell
#[cfg(target_family = "unix")]
pub fn get_zsh_prompt(env_name: &str) -> String {
    format!("export PS1=\"({}) $PS1\"", env_name)
}

/// Set default pixi prompt for the fish shell
#[cfg(target_family = "unix")]
pub fn get_fish_prompt(env_name: &str) -> String {
    format!(
        "functions -c fish_prompt old_fish_prompt; \
         function fish_prompt; \
             echo \"({}) $(old_fish_prompt)\"; \
         end;",
        env_name
    )
}

/// Set default pixi prompt for the xonsh shell
#[cfg(target_family = "unix")]
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
#[cfg(target_family = "windows")]
pub fn get_cmd_prompt(env_name: &str) -> String {
    format!(r"@PROMPT ({}) $P$G", env_name)
}
