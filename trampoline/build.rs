//! Embeds a Windows application manifest into the pixi trampoline binary.
//!
//! This enables:
//! - Long path awareness (paths > 260 characters), requires `LongPathsEnabled`
//!   registry key to also be set.
//! - Declared Windows 7-10+ compatibility to avoid legacy compat layers.
//! - Standard invoker execution levels to disable UAC virtualization.
//!
//! Reference: https://learn.microsoft.com/en-us/windows/win32/sbscs/application-manifests
//! Based on: https://github.com/astral-sh/uv/pull/16894
use embed_manifest::manifest::{ActiveCodePage, ExecutionLevel, Setting, SupportedOS};
use embed_manifest::{embed_manifest, empty_manifest};

fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        let version = env!("CARGO_PKG_VERSION");
        let [major, minor, patch] = version
            .splitn(3, '.')
            .map(str::parse)
            .collect::<Result<Vec<u16>, _>>()
            .ok()
            .and_then(|v| v.try_into().ok())
            .expect("pixi_trampoline version must be in x.y.z format");
        let manifest = empty_manifest()
            .name("pixi_trampoline")
            .version(major, minor, patch, 0)
            .active_code_page(ActiveCodePage::System)
            .supported_os(SupportedOS::Windows7..=SupportedOS::Windows10)
            .requested_execution_level(ExecutionLevel::AsInvoker)
            .long_path_aware(Setting::Enabled);
        embed_manifest(manifest).expect("unable to embed manifest");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
