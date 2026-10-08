//! Pixi-specific defaults for Rattler's authentication CLI.

use clap::{FromArgMatches, builder::ArgPredicate};

const BASILISK_AUDIENCE: &str = "https://api.basilisk.prefix.dev";

#[derive(Debug)]
pub struct Args {
    pub(crate) inner: rattler::cli::auth::Args,
}

impl FromArgMatches for Args {
    fn from_arg_matches(matches: &clap::ArgMatches) -> Result<Self, clap::Error> {
        Ok(Self {
            inner: rattler::cli::auth::Args::from_arg_matches(matches)?,
        })
    }

    fn update_from_arg_matches(&mut self, matches: &clap::ArgMatches) -> Result<(), clap::Error> {
        self.inner.update_from_arg_matches(matches)
    }
}

impl clap::Args for Args {
    fn augment_args(command: clap::Command) -> clap::Command {
        with_pixi_defaults(rattler::cli::auth::Args::augment_args(command))
    }

    fn augment_args_for_update(command: clap::Command) -> clap::Command {
        with_pixi_defaults(rattler::cli::auth::Args::augment_args_for_update(command))
    }
}

fn with_pixi_defaults(command: clap::Command) -> clap::Command {
    command.mut_subcommands(|login| {
        if login.get_name() != "login" {
            return login;
        }
        login.mut_arg("oauth_audience", |audience| {
            // Clap uses the first matching condition. Explicit audiences always
            // win; other auth methods and custom OAuth clients get no default.
            audience.help("OAuth audience to request (provider-specific). Defaults to https://api.basilisk.prefix.dev for the built-in prefix.dev login.").default_value_ifs([
                ("token", ArgPredicate::IsPresent, None),
                ("username", ArgPredicate::IsPresent, None),
                ("password", ArgPredicate::IsPresent, None),
                ("conda_token", ArgPredicate::IsPresent, None),
                ("s3_access_key_id", ArgPredicate::IsPresent, None),
                (
                    "workload_identity",
                    ArgPredicate::Equals("true".into()),
                    None,
                ),
                ("oauth_issuer_url", ArgPredicate::IsPresent, None),
                ("oauth_client_id", ArgPredicate::IsPresent, None),
                ("oauth_client_secret", ArgPredicate::IsPresent, None),
                (
                    "host",
                    ArgPredicate::Equals("prefix.dev".into()),
                    Some(BASILISK_AUDIENCE),
                ),
                (
                    "host",
                    ArgPredicate::Equals("https://prefix.dev".into()),
                    Some(BASILISK_AUDIENCE),
                ),
                (
                    "host",
                    ArgPredicate::Equals("prefix.dev/".into()),
                    Some(BASILISK_AUDIENCE),
                ),
                (
                    "host",
                    ArgPredicate::Equals("https://prefix.dev/".into()),
                    Some(BASILISK_AUDIENCE),
                ),
            ])
        })
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn audience(host: &str, options: &[&str]) -> Option<String> {
        let command = crate::Args::command();
        command.clone().debug_assert();
        let matches = command
            .try_get_matches_from([&["pixi", "auth", "login", host][..], options].concat())
            .unwrap();
        // Exercise conversion to the real Rattler arguments as well as parsing.
        crate::Args::from_arg_matches(&matches).unwrap();
        matches
            .subcommand_matches("auth")
            .unwrap()
            .subcommand_matches("login")
            .unwrap()
            .get_one::<String>("oauth_audience")
            .cloned()
    }

    #[test]
    fn prefix_login_requests_basilisk_audience() {
        for host in [
            "prefix.dev",
            "https://prefix.dev",
            "prefix.dev/",
            "https://prefix.dev/",
        ] {
            assert_eq!(audience(host, &[]).as_deref(), Some(BASILISK_AUDIENCE));
            assert_eq!(
                audience(host, &["--oauth"]).as_deref(),
                Some(BASILISK_AUDIENCE)
            );
        }
    }

    #[test]
    fn other_hosts_and_custom_clients_have_no_default() {
        for host in [
            "beta.prefix.dev",
            "repo.prefix.dev",
            "example.com",
            "prefix.dev.example.com",
            "http://prefix.dev",
            "https://prefix.dev:8443",
        ] {
            assert_eq!(audience(host, &[]), None, "{host}");
        }
        for flag in [
            "--oauth-issuer-url",
            "--oauth-client-id",
            "--oauth-client-secret",
        ] {
            assert_eq!(
                audience("prefix.dev", &["--oauth", flag, "custom"]),
                None,
                "{flag}"
            );
        }
    }

    #[test]
    fn explicit_audience_wins() {
        for host in ["prefix.dev", "example.com"] {
            assert_eq!(
                audience(host, &["--oauth-audience", "https://custom.example"]).as_deref(),
                Some("https://custom.example")
            );
        }
    }

    #[test]
    fn other_auth_methods_have_no_default() {
        for options in [
            vec!["--token", "token"],
            vec!["--username", "user", "--password", "password"],
            vec!["--conda-token", "token"],
            vec![
                "--s3-access-key-id",
                "key",
                "--s3-secret-access-key",
                "secret",
            ],
            vec!["--workload-identity"],
        ] {
            assert_eq!(audience("prefix.dev", &options), None, "{options:?}");
        }
    }
}
