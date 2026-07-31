use std::{
    io::Write,
    path::{Path, PathBuf},
};

use miette::{IntoDiagnostic, WrapErr};
use pixi_config::Config;
use pixi_manifest::script::ScriptManifest;
use pixi_utils::reqwest::build_reqwest_clients;
use tempfile::NamedTempFile;
use url::Url;

#[derive(Debug)]
pub(crate) enum RunScriptInput {
    Local(PathBuf),
    Remote(Url),
}

impl RunScriptInput {
    pub(crate) fn classify(input: &Path) -> Self {
        let Some(input) = input.to_str() else {
            return Self::Local(input.to_owned());
        };
        match Url::parse(input) {
            Ok(url) if matches!(url.scheme(), "http" | "https") => Self::Remote(url),
            _ => Self::Local(input.into()),
        }
    }
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
    let mut response = client.get(original_url.clone()).send().await.map_err(|_| {
        miette::miette!("failed to download remote script from {safe_original_url}")
    })?;
    let final_url = response.url().clone();
    let status = response.status();
    if !status.is_success() {
        return Err(miette::miette!(
            "failed to download remote script from {safe_original_url}: server returned {status}"
        ));
    }

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
    use super::{RunScriptInput, friendly_name, safe_url};
    use std::path::Path;
    use url::Url;

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
}
