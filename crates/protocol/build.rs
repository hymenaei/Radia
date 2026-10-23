#[path = "build/mod.rs"]
mod build;

use std::{env, fs, path::PathBuf};

use sha2::{Digest, Sha256};

const PINNED_TEMPLATE_COMMIT: &str = "d5445ef1ccb26864236653bbe91d6878bb2277fc";
const PINNED_TEMPLATE_SHA256: &str = "f7da9162ae7ab4424bb5e9d285a537ef4b251d2f9942246f934d9af9bff1f3c8";

fn main() {
    println!("cargo:rerun-if-changed=build/message_template.msg");

    let source = fs::read_to_string("build/message_template.msg").expect("read vendored Second Life message template");
    let normalized_source = source.replace("\r\n", "\n").replace('\r', "\n");
    let actual_hash = utils::hex_lower(&Sha256::digest(normalized_source.as_bytes()));
    if actual_hash != PINNED_TEMPLATE_SHA256 {
        panic!(
            "vendored message template from upstream commit {PINNED_TEMPLATE_COMMIT} has normalized SHA-256 {actual_hash}, expected {PINNED_TEMPLATE_SHA256}; review the template and update its pin intentionally"
        );
    }

    let template = build::template::parse("build/message_template.msg", &source).unwrap_or_else(|error| panic!("{error}"));
    let generated = build::generate::generate(&template);
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR")).join("schema.rs");

    fs::write(output, generated).expect("write generated schema metadata");
}
