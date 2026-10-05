//! Best-effort anonymous ping after installing or updating pixi.
//!
//! See `docs/reference/telemetry.md` for what is sent and how to opt out.

use std::path::PathBuf;
use std::time::Duration;

use pixi_consts::consts;
use pixi_utils::reqwest::build_reqwest_clients;
use reqwest_middleware::ClientWithMiddleware;

const OPT_OUT_HINT: &str = "Set PIXI_NO_TELEMETRY=1 or DO_NOT_TRACK=1 to opt out. \
                            See https://pixi.sh/latest/reference/telemetry/";

/// Send the anonymous installation ping. Called by the install scripts right
/// after installing the binary.
#[derive(Debug, clap::Parser)]
pub struct Args {}

#[derive(Debug, Clone, Copy)]
enum PingEvent {
    Install,
    SelfUpdate,
}

impl PingEvent {
    fn as_str(self) -> &'static str {
        match self {
            PingEvent::Install => "install",
            PingEvent::SelfUpdate => "self-update",
        }
    }
}

pub async fn execute(_args: Args) -> miette::Result<()> {
    if telemetry_disabled() {
        return Ok(());
    }

    // Printed to stdout so the install scripts can capture it and only show it
    // when the command succeeded.
    println!(
        "Sending an anonymous installation ping to prefix.dev (version, OS, arch). {OPT_OUT_HINT}"
    );
    mark_notice_shown();

    let client = build_reqwest_clients(None, None)?.1;
    send_ping(&client, PingEvent::Install, consts::PIXI_VERSION).await;
    Ok(())
}

/// Ping after a successful `pixi self-update`.
///
/// If the user has not seen the telemetry notice yet (e.g. pixi was not
/// installed through the install scripts), only the notice is printed and
/// recorded, so they can opt out before any ping is sent. If the notice cannot
/// be recorded, no ping is sent.
pub async fn self_update_ping(client: &ClientWithMiddleware, version: &str, is_quiet: bool) {
    if telemetry_disabled() {
        return;
    }

    let Some(marker) = notice_marker() else {
        return;
    };
    if !marker.exists() {
        // Without the notice being visible we must not record it as shown.
        if !is_quiet {
            eprintln!(
                "pixi self-update sends an anonymous ping (version, OS, arch) to prefix.dev \
                 after each update, starting with the next one. {OPT_OUT_HINT}"
            );
            mark_notice_shown();
        }
        return;
    }

    send_ping(client, PingEvent::SelfUpdate, version).await;
}

/// Empty values count as unset.
fn telemetry_disabled() -> bool {
    let is_set = |name: &str| std::env::var_os(name).is_some_and(|v| !v.is_empty());
    is_set("PIXI_NO_TELEMETRY") || is_set("DO_NOT_TRACK")
}

fn notice_marker() -> Option<PathBuf> {
    pixi_config::get_cache_dir()
        .ok()
        .map(|dir| dir.join(consts::TELEMETRY_NOTICE_MARKER))
}

fn mark_notice_shown() {
    let Some(marker) = notice_marker() else {
        return;
    };
    if let Some(parent) = marker.parent() {
        let _ = fs_err::create_dir_all(parent);
    }
    let _ = fs_err::write(&marker, "");
}

async fn send_ping(client: &ClientWithMiddleware, event: PingEvent, version: &str) {
    // Encode the metadata as a synthetic page URL. Scarf reports on the `Page`
    // dimension (normally inferred from the referrer), so each event/version/
    // platform combination shows up as its own page in the dashboard.
    let page = format!(
        "https://pixi.sh/ping/{}/{}/{}-{}",
        event.as_str(),
        version.trim_start_matches('v'),
        std::env::consts::OS,
        std::env::consts::ARCH,
    );

    // Fire-and-forget: we deliberately ignore the result. A failed ping must
    // never affect the install or update, so any error (timeout, network,
    // HTTP) is dropped. The client's default timeout is minutes, hence the
    // explicit short one.
    let _ = client
        .get(consts::INSTALL_PING_URL)
        .query(&[
            ("x-pxid", consts::INSTALL_PING_PXID),
            ("Page", page.as_str()),
        ])
        .timeout(Duration::from_secs(3))
        .send()
        .await;
}
