#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    pub max_bytes: usize,
    pub max_depth: usize,
    pub max_values: usize,
    pub max_container_items: usize,
    pub max_string_bytes: usize,
    pub max_map_key_bytes: usize,
    pub max_binary_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_bytes: 32 * 1024 * 1024,
            max_depth: 96,
            max_values: 262_144,
            max_container_items: 262_144,
            max_string_bytes: 4 * 1024 * 1024,
            max_map_key_bytes: 64 * 1024,
            max_binary_bytes: 32 * 1024 * 1024,
        }
    }
}
