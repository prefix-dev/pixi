use clap::Parser;
use rattler_networking::{Authentication, AuthenticationStorage};

fn default_authentication_storage() -> AuthenticationStorage {
    AuthenticationStorage::new("rattler", &dirs::home_dir().unwrap().join(".rattler"))
}

#[derive(Parser, Debug)]
pub struct LoginArgs {
    /// The host to authenticate with (e.g. repo.prefix.dev)
    host: String,

    /// The token to use (for authentication with prefix.dev)
    #[clap(long)]
    token: Option<String>,

    /// The username to use (for basic HTTP authentication)
    #[clap(long)]
    username: Option<String>,

    /// The password to use (for basic HTTP authentication)
    #[clap(long)]
    password: Option<String>,

    /// The token to use on anaconda.org / quetz authentication
    #[clap(long)]
    conda_token: Option<String>,
}

#[derive(Parser, Debug)]
struct LogoutArgs {
    /// The host to remove authentication for
    host: String,
}

#[derive(Parser, Debug)]
enum Subcommand {
    /// Store authentication information for a given host
    Login(LoginArgs),
    /// Remove authentication information for a given host
    Logout(LogoutArgs),
}

/// Login to prefix.dev or anaconda.org servers to access private channels
#[derive(Parser, Debug)]
pub struct Args {
    #[clap(subcommand)]
    subcommand: Subcommand,
}

fn get_url(url: &str) -> anyhow::Result<String> {
    // parse as url and extract host without scheme or port
    let host = if url.contains("://") {
        url::Url::parse(url)?.host_str().unwrap().to_string()
    } else {
        url.to_string()
    };

    let host = if host.matches('.').count() == 1 {
        // use wildcard for top-level domains
        format!("*.{}", host)
    } else {
        host
    };

    Ok(host)
}

fn login(args: LoginArgs, storage: AuthenticationStorage) -> anyhow::Result<()> {
    let host = get_url(&args.host)?;
    println!("Authenticating with {}", host);

    let auth = if let Some(conda_token) = args.conda_token {
        Authentication::CondaToken(conda_token)
    } else if let Some(username) = args.username {
        if args.password.is_none() {
            anyhow::bail!("Password must be provided when using basic authentication");
        }
        let password = args.password.unwrap();
        Authentication::BasicHTTP { username, password }
    } else if let Some(token) = args.token {
        Authentication::BearerToken(token)
    } else {
        anyhow::bail!("No authentication method provided");
    };

    if host.contains("prefix.dev") && !matches!(auth, Authentication::BearerToken(_)) {
        anyhow::bail!(
            "Authentication with prefix.dev requires a token. Use `--token` to provide one."
        );
    }

    if host.contains("anaconda.org") && !matches!(auth, Authentication::CondaToken(_)) {
        anyhow::bail!("Authentication with anaconda.org requires a conda token. Use `--conda-token` to provide one.");
    }

    storage.store(&host, &auth)?;
    Ok(())
}

fn logout(args: LogoutArgs, storage: AuthenticationStorage) -> anyhow::Result<()> {
    let host = get_url(&args.host)?;

    println!("Removing authentication for {}", host);

    storage.delete(&host)?;
    Ok(())
}

pub async fn execute(args: Args) -> anyhow::Result<()> {
    let storage = default_authentication_storage();

    match args.subcommand {
        Subcommand::Login(args) => login(args, storage),
        Subcommand::Logout(args) => logout(args, storage),
    }
}
