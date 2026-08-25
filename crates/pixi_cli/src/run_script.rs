use std::{
    ffi::OsString,
    io::Write,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

use deno_task_shell::{ExecutableCommand, FutureExecuteResult, ShellCommand, ShellCommandContext};
use miette::{IntoDiagnostic, WrapErr};
use pixi_config::Config;
use pixi_manifest::script::ScriptManifest;
use pixi_utils::reqwest::build_reqwest_clients;
use reqwest::header::ACCEPT;
use reqwest_middleware::{ClientWithMiddleware, RequestBuilder};
use serde::Deserialize;
use tempfile::NamedTempFile;
use url::Url;

#[derive(Debug)]
pub(crate) enum RunScriptInput {
    Local(PathBuf),
    Remote(Url),
    Stdin,
}

impl RunScriptInput {
    pub(crate) fn classify(input: &Path) -> Self {
        if input == Path::new("-") {
            return Self::Stdin;
        }
        let Some(input) = input.to_str() else {
            return Self::Local(input.to_owned());
        };
        match Url::parse(input) {
            Ok(url) if matches!(url.scheme(), "http" | "https") => Self::Remote(url),
            _ => Self::Local(input.into()),
        }
    }
}

/// Separates transient environments by input kind.
pub(crate) fn transient_script_cache_key(kind: &[u8], identity: &[u8]) -> Vec<u8> {
    let mut key = Vec::with_capacity(kind.len() + identity.len() + 1);
    key.extend_from_slice(kind);
    key.push(0);
    key.extend_from_slice(identity);
    key
}

pub(crate) const STDIN_SCRIPT_COMMAND: &str = "__pixi_stdin_script__";

#[derive(Clone)]
pub(crate) struct StdinScriptCommand {
    contents: Arc<str>,
}

impl StdinScriptCommand {
    pub(crate) fn new(contents: String) -> Self {
        Self {
            contents: contents.into(),
        }
    }

    pub(crate) fn shell_command(&self) -> Rc<dyn ShellCommand> {
        Rc::new(self.clone())
    }
}

impl ShellCommand for StdinScriptCommand {
    fn execute(&self, mut context: ShellCommandContext) -> FutureExecuteResult {
        context.args.splice(
            0..0,
            [OsString::from("-c"), OsString::from(self.contents.as_ref())],
        );
        ExecutableCommand::new("python".to_owned(), PathBuf::from("python")).execute(context)
    }
}

pub(crate) struct PreparedStdinScript {
    pub(crate) manifest: ScriptManifest,
    pub(crate) command: StdinScriptCommand,
}

pub(crate) fn prepare_stdin_script(
    contents: Vec<u8>,
    root: &Path,
) -> miette::Result<PreparedStdinScript> {
    let manifest = ScriptManifest::from_source_with_context(
        root.join("stdin.py"),
        &contents,
        "<stdin>",
        root.to_owned(),
        "stdin",
    )?
    .ok_or_else(|| miette::miette!("stdin does not contain a PEP 723 metadata block"))?;
    let contents =
        String::from_utf8(contents).expect("ScriptManifest validates the complete script as UTF-8");
    Ok(PreparedStdinScript {
        manifest,
        command: StdinScriptCommand::new(contents),
    })
}

pub(crate) struct PreparedRemoteScript {
    pub(crate) file: NamedTempFile,
    pub(crate) manifest: ScriptManifest,
    pub(crate) original_url: Url,
    pub(crate) cache_name: String,
}

pub(crate) async fn prepare_remote_script(
    original_url: Url,
    config: &Config,
    root: &Path,
) -> miette::Result<PreparedRemoteScript> {
    let safe_original_url = safe_url(&original_url);
    let (_, client) = build_reqwest_clients(Some(config), None)?;
    let mut response = send_success(
        client.get(original_url.clone()),
        &safe_original_url,
        "download",
    )
    .await?;
    if response.url().host_str() == Some("gist.github.com") {
        let raw_url = resolve_gist(&client, response.url(), &safe_original_url).await?;
        response = send_success(client.get(raw_url), &safe_original_url, "download").await?;
    }

    let final_url = response.url().clone();
    let cache_name = friendly_name(&final_url);
    let mut file = tempfile::Builder::new()
        .prefix(&format!("{cache_name}-"))
        .suffix(".py")
        .tempfile()
        .into_diagnostic()
        .wrap_err("failed to create a temporary file for the remote script")?;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| miette::miette!("failed to download remote script from {safe_original_url}"))?
    {
        file.write_all(&chunk)
            .into_diagnostic()
            .wrap_err("failed to write the downloaded script to its temporary file")?;
    }
    file.flush()
        .into_diagnostic()
        .wrap_err("failed to flush the downloaded script")?;

    let contents = fs_err::read(file.path())
        .into_diagnostic()
        .wrap_err("failed to read the downloaded script")?;
    let manifest = ScriptManifest::from_source_with_context(
        file.path().to_owned(),
        &contents,
        safe_original_url.clone(),
        root.to_owned(),
        cache_name.clone(),
    )?
    .ok_or_else(|| {
        miette::miette!(
            help =
                "Download the script and initialize it locally with `pixi init --script <PATH>`.",
            "the remote script at {safe_original_url} does not contain a PEP 723 metadata block"
        )
    })?;

    Ok(PreparedRemoteScript {
        file,
        manifest,
        original_url,
        cache_name,
    })
}

async fn send_success(
    request: RequestBuilder,
    safe_original_url: &str,
    action: &str,
) -> miette::Result<reqwest::Response> {
    let response = request.send().await.map_err(|_| {
        miette::miette!("failed to {action} remote script from {safe_original_url}")
    })?;
    let status = response.status();
    if !status.is_success() {
        return Err(miette::miette!(
            "failed to {action} remote script from {safe_original_url}: server returned {status}"
        ));
    }
    Ok(response)
}

#[derive(Debug, Deserialize)]
struct Gist {
    files: indexmap::IndexMap<String, GistFile>,
}

#[derive(Debug, Deserialize)]
struct GistFile {
    filename: String,
    raw_url: Url,
}

async fn resolve_gist(
    client: &ClientWithMiddleware,
    gist_url: &Url,
    safe_original_url: &str,
) -> miette::Result<Url> {
    let gist_id = gist_id(gist_url).ok_or_else(|| {
        miette::miette!(
            "could not determine the Gist ID from {}",
            safe_url(gist_url)
        )
    })?;
    let api_url = Url::parse(&format!("https://api.github.com/gists/{gist_id}"))
        .expect("the GitHub Gist API URL is valid");
    resolve_gist_at(client, api_url, safe_original_url).await
}

async fn resolve_gist_at(
    client: &ClientWithMiddleware,
    api_url: Url,
    safe_original_url: &str,
) -> miette::Result<Url> {
    let mut request = client
        .get(api_url)
        .header(ACCEPT, "application/vnd.github+json");
    if let Ok(token) = std::env::var("PIXI_GITHUB_TOKEN") {
        request = request.bearer_auth(token);
    }
    let response = send_success(request, safe_original_url, "resolve GitHub Gist").await?;
    let body = response.bytes().await.map_err(|_| {
        miette::miette!("failed to read the GitHub Gist response for {safe_original_url}")
    })?;
    let gist = serde_json::from_slice::<Gist>(&body).map_err(|_| {
        miette::miette!("the GitHub API returned an invalid Gist response for {safe_original_url}")
    })?;
    select_gist_file(&gist).cloned().ok_or_else(|| {
        miette::miette!("the GitHub Gist at {safe_original_url} does not contain any files")
    })
}

fn gist_id(url: &Url) -> Option<&str> {
    let mut segments = url.path_segments()?.filter(|segment| !segment.is_empty());
    segments.next()?;
    segments.next()
}

fn select_gist_file(gist: &Gist) -> Option<&Url> {
    gist.files
        .values()
        .find(|file| file.filename.to_ascii_lowercase().ends_with(".py"))
        .or_else(|| gist.files.values().next())
        .map(|file| &file.raw_url)
}

pub(crate) fn safe_url(url: &Url) -> String {
    let mut safe = url.clone();
    if !safe.username().is_empty() {
        let _ = safe.set_username("***");
    }
    if safe.password().is_some() {
        let _ = safe.set_password(Some("***"));
    }
    safe.to_string()
}

fn friendly_name(url: &Url) -> String {
    let name = url
        .path_segments()
        .and_then(|mut segments| segments.rfind(|segment| !segment.is_empty()))
        .unwrap_or("script");
    let name = name.strip_suffix(".py").unwrap_or(name);
    let name = name
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_') {
                byte as char
            } else {
                '-'
            }
        })
        .collect::<String>();
    let name = name.trim_matches('-');
    if name.is_empty() {
        "script".to_owned()
    } else {
        name.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Gist, RunScriptInput, friendly_name, gist_id, resolve_gist_at, safe_url, select_gist_file,
        transient_script_cache_key,
    };
    use std::{
        io::{Read, Write},
        net::TcpListener,
        path::Path,
        sync::mpsc,
        thread,
    };
    use url::Url;

    fn serve_once(
        status: &str,
        body: &str,
    ) -> (Url, mpsc::Receiver<String>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = mpsc::channel();
        let status = status.to_owned();
        let body = body.to_owned();
        let thread = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            loop {
                let mut chunk = [0; 1024];
                let length = stream.read(&mut chunk).unwrap();
                request.extend_from_slice(&chunk[..length]);
                if length == 0 || request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let _ = sender.send(String::from_utf8_lossy(&request).into_owned());
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        (
            Url::parse(&format!("http://{address}/gists/abc123")).unwrap(),
            receiver,
            thread,
        )
    }

    #[test]
    fn classifies_only_http_urls_as_remote() {
        assert!(matches!(
            RunScriptInput::classify(Path::new("https://example.com/script")),
            RunScriptInput::Remote(_)
        ));
        assert!(matches!(
            RunScriptInput::classify(Path::new("http://example.com/script.py")),
            RunScriptInput::Remote(_)
        ));
        assert!(matches!(
            RunScriptInput::classify(Path::new("file:///tmp/script.py")),
            RunScriptInput::Local(_)
        ));
        assert!(matches!(
            RunScriptInput::classify(Path::new("script.py")),
            RunScriptInput::Local(_)
        ));
        assert!(matches!(
            RunScriptInput::classify(Path::new("-")),
            RunScriptInput::Stdin
        ));
    }

    #[test]
    fn transient_cache_keys_include_the_input_context() {
        let first = transient_script_cache_key(b"stdin", b"metadata");
        let same = transient_script_cache_key(b"stdin", b"metadata");
        let other_kind = transient_script_cache_key(b"remote", b"metadata");
        let other_identity = transient_script_cache_key(b"stdin", b"other metadata");

        assert_eq!(first, same);
        assert_ne!(first, other_kind);
        assert_ne!(first, other_identity);
    }

    #[test]
    fn derives_safe_friendly_names() {
        assert_eq!(
            friendly_name(&Url::parse("https://example.com/path/example.py").unwrap()),
            "example"
        );
        assert_eq!(
            friendly_name(&Url::parse("https://example.com/path/no%20spaces").unwrap()),
            "no-20spaces"
        );
        assert_eq!(
            friendly_name(&Url::parse("https://example.com/").unwrap()),
            "script"
        );
    }

    #[test]
    fn redacts_url_credentials() {
        assert_eq!(
            safe_url(&Url::parse("https://user:secret@example.com/script.py").unwrap()),
            "https://***:***@example.com/script.py"
        );
    }

    #[test]
    fn extracts_gist_ids_from_normal_gist_urls() {
        assert_eq!(
            gist_id(&Url::parse("https://gist.github.com/user/abc123").unwrap()),
            Some("abc123")
        );
        assert_eq!(
            gist_id(&Url::parse("https://gist.github.com/abc123").unwrap()),
            None
        );
    }

    #[test]
    fn selects_the_first_python_file_from_a_gist() {
        let gist: Gist = serde_json::from_str(
            r#"{
                "files": {
                    "README.md": {
                        "filename": "README.md",
                        "raw_url": "https://example.com/readme"
                    },
                    "FIRST.PY": {
                        "filename": "FIRST.PY",
                        "raw_url": "https://example.com/first"
                    },
                    "second.py": {
                        "filename": "second.py",
                        "raw_url": "https://example.com/second"
                    }
                }
            }"#,
        )
        .unwrap();
        assert_eq!(
            select_gist_file(&gist).unwrap().as_str(),
            "https://example.com/first"
        );
    }

    #[test]
    fn falls_back_to_the_first_gist_file() {
        let gist: Gist = serde_json::from_str(
            r#"{
                "files": {
                    "README.md": {
                        "filename": "README.md",
                        "raw_url": "https://example.com/readme"
                    },
                    "script.rb": {
                        "filename": "script.rb",
                        "raw_url": "https://example.com/ruby"
                    }
                }
            }"#,
        )
        .unwrap();
        assert_eq!(
            select_gist_file(&gist).unwrap().as_str(),
            "https://example.com/readme"
        );
    }

    #[tokio::test]
    async fn authenticates_the_gist_api_request() {
        let (api_url, request, server) = serve_once(
            "200 OK",
            r#"{
                "files": {
                    "script.py": {
                        "filename": "script.py",
                        "raw_url": "https://raw.example.com/script.py"
                    }
                }
            }"#,
        );
        let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new()).build();
        let raw_url = temp_env::async_with_vars(
            [("PIXI_GITHUB_TOKEN", Some("gist-secret"))],
            resolve_gist_at(&client, api_url, "https://gist.github.com/user/abc123"),
        )
        .await
        .unwrap();
        let request = request.recv().unwrap();
        server.join().unwrap();

        assert_eq!(raw_url.as_str(), "https://raw.example.com/script.py");
        assert!(request.contains("authorization: Bearer gist-secret"));
        assert!(request.contains("accept: application/vnd.github+json"));
    }

    #[test]
    fn rejects_an_empty_gist() {
        let gist: Gist = serde_json::from_str(r#"{"files": {}}"#).unwrap();
        assert!(select_gist_file(&gist).is_none());
    }

    #[tokio::test]
    async fn reports_gist_api_http_errors_at_the_api_boundary() {
        let (api_url, _, server) = serve_once("403 Forbidden", "");
        let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new()).build();
        let error = temp_env::async_with_vars(
            [("PIXI_GITHUB_TOKEN", None::<&str>)],
            resolve_gist_at(&client, api_url, "https://gist.github.com/user/abc123"),
        )
        .await
        .unwrap_err();
        server.join().unwrap();

        let error = error.to_string();
        assert!(error.contains("resolve GitHub Gist"));
        assert!(error.contains("403 Forbidden"));
    }
}
