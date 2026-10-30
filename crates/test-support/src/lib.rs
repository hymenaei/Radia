pub mod allocation;

pub mod lludp;
pub mod process;
pub mod texture;

pub fn parse_hex_fixture(source: &str) -> Result<Vec<u8>, String> {
    source
        .split_ascii_whitespace()
        .enumerate()
        .map(|(index, token)| {
            if token.len() != 2 {
                return Err(format!("fixture token {} must have two hex digits", index + 1));
            }

            if !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(format!("fixture token {} is not hexadecimal", index + 1));
            }

            u8::from_str_radix(token, 16).map_err(|_| format!("fixture token {} is not hexadecimal", index + 1))
        })
        .collect()
}
