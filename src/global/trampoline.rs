const TRAMPOLINE_BIN: &[u8] = include_bytes!(env!("TRAMPOLINE_PATH"));

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
