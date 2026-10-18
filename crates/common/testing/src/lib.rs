//! Literal LLUDP fixtures and parsing helpers for tests.

/// Independently assembled LLUDP packet fixtures for protocol tests.
pub mod lludp;

/// Parses whitespace-separated hexadecimal bytes from a fixture.
///
/// # Errors
///
/// Returns an error if a token is not exactly two ASCII hexadecimal digits.
/// The message identifies the token's one-based position.
///
/// # Examples
///
/// ```
/// let bytes = testing::parse_hex_fixture("01 ff")?;
/// assert_eq!(bytes, [1, 255]);
/// # Ok::<(), String>(())
/// ```
pub fn parse_hex_fixture(source: &str) -> Result<Vec<u8>, String> {
    source
        .split_ascii_whitespace()
        .enumerate()
        .map(|(index, token)| {
            if token.len() != 2 {
                return Err(format!("fixture token {} must have two hex digits", index + 1));
            }

            u8::from_str_radix(token, 16).map_err(|_| format!("fixture token {} is not hexadecimal", index + 1))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{lludp::vectors::ALL, parse_hex_fixture};
    use std::fs::read_dir;
    use std::path::Path;

    #[test]
    fn parses_hex_bytes() {
        assert_eq!(parse_hex_fixture(" 0a\tFF\n"), Ok(vec![0x0a, 0xff]));
    }

    #[test]
    fn rejects_malformed_hex_tokens() {
        assert_eq!(parse_hex_fixture("01 f"), Err("fixture token 2 must have two hex digits".to_owned()));
        assert_eq!(parse_hex_fixture("01 gg"), Err("fixture token 2 is not hexadecimal".to_owned()));
    }

    #[test]
    fn lludp_vectors_are_hex() {
        for (name, source) in ALL {
            let bytes = parse_hex_fixture(source).unwrap_or_else(|error| panic!("{name} is invalid: {error}"));
            assert!(!bytes.is_empty(), "{name} must not be empty");
        }
    }

    #[test]
    fn lludp_vector_index_covers_every_checked_in_fixture() {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/lludp");
        let mut files: Vec<_> = read_dir(directory)
            .expect("read LLUDP fixtures")
            .map(|entry| entry.expect("read LLUDP fixture entry").file_name().into_string().expect("fixture filename is UTF-8"))
            .filter(|name| name.ends_with(".hex"))
            .collect();
        let mut indexed: Vec<_> = ALL.iter().map(|(name, _)| (*name).to_owned()).collect();
        files.sort();
        indexed.sort();

        assert_eq!(indexed, files, "LLUDP fixture index and checked-in files differ");
    }
}
