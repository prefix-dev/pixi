#[cfg(target_arch = "aarch64")]
#[cfg(target_os = "macos")]
const TRAMPOLINE_BIN: &[u8] =
    include_bytes!("../../crates/pixi_trampoline/trampolines/pixi-trampoline-aarch64-apple-darwin");

#[cfg(target_arch = "aarch64")]
#[cfg(target_os = "windows")]
const TRAMPOLINE_BIN: &[u8] = include_bytes!(
    "../../crates/pixi_trampoline/trampolines/pixi-trampoline-aarch64-pc-windows-msvc.exe"
);

#[cfg(target_arch = "aarch64")]
#[cfg(target_os = "linux")]
const TRAMPOLINE_BIN: &[u8] = include_bytes!(
    "../../crates/pixi_trampoline/trampolines/pixi-trampoline-aarch64-unknown-linux-musl"
);

#[cfg(target_arch = "x86_64")]
#[cfg(target_os = "macos")]
const TRAMPOLINE_BIN: &[u8] =
    include_bytes!("../../crates/pixi_trampoline/trampolines/pixi-trampoline-x86_64-apple-darwin");

#[cfg(target_arch = "x86_64")]
#[cfg(target_os = "windows")]
const TRAMPOLINE_BIN: &[u8] = include_bytes!(
    "../../crates/pixi_trampoline/trampolines/pixi-trampoline-x86_64-pc-windows-msvc.exe"
);

#[cfg(target_arch = "x86_64")]
#[cfg(target_os = "linux")]
const TRAMPOLINE_BIN: &[u8] = include_bytes!(
    "../../crates/pixi_trampoline/trampolines/pixi-trampoline-x86_64-unknown-linux-musl"
);

#[allow(dead_code)]
pub struct Trampoline {
    binary_data: &'static [u8],
}

#[allow(dead_code)]
impl Trampoline {
    pub fn new() -> Self {
        let binary_data = TRAMPOLINE_BIN;
        Trampoline { binary_data }
    }

    pub fn get_binary_size(&self) -> usize {
        self.binary_data.len()
    }

    // Add more methods as needed for your specific use case
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trampoline_creation() {
        let trampoline = Trampoline::new();
        assert!(
            trampoline.get_binary_size() > 0,
            "Binary should not be empty"
        );
    }
}
