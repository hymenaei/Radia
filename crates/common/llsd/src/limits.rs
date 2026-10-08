/// Input and decoded-value limits applied by LLSD parsers.
///
/// Defaults cap input at 32 MiB, nesting at 64 levels, values and container items at 262,144, strings at 4 MiB, map keys at
/// 64 KiB, and binary values at 32 MiB.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    pub max_input_bytes: usize,
    /// Maximum number of nested arrays and maps.
    pub max_depth: usize,
    pub max_values: usize,
    pub max_container_items: usize,
    /// Maximum UTF-8 byte length of one decoded string.
    pub max_string_bytes: usize,
    /// Maximum UTF-8 byte length of one map key.
    pub max_map_key_bytes: usize,
    pub max_binary_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_input_bytes: 32 * 1024 * 1024,
            max_depth: 64,
            max_values: 262_144,
            max_container_items: 262_144,
            max_string_bytes: 4 * 1024 * 1024,
            max_map_key_bytes: 64 * 1024,
            max_binary_bytes: 32 * 1024 * 1024,
        }
    }
}
