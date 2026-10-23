use embed_manifest::manifest::Setting;
use embed_manifest::{embed_manifest, empty_manifest};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    // Build scripts run on the host, so check Cargo's target platform.
    if std::env::var_os("CARGO_CFG_WINDOWS").is_none() {
        return;
    }

    let major = env!("CARGO_PKG_VERSION_MAJOR")
        .parse()
        .expect("pixi major version must fit in a Windows manifest");
    let minor = env!("CARGO_PKG_VERSION_MINOR")
        .parse()
        .expect("pixi minor version must fit in a Windows manifest");
    let patch = env!("CARGO_PKG_VERSION_PATCH")
        .parse()
        .expect("pixi patch version must fit in a Windows manifest");

    let manifest = empty_manifest()
        .name("pixi")
        .version(major, minor, patch, 0)
        .long_path_aware(Setting::Enabled);
    embed_manifest(manifest).expect("unable to embed the Windows application manifest");
}
