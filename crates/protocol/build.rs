#[path = "build/mod.rs"]
mod build;

use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build/message_template.msg");

    let source = fs::read_to_string("build/message_template.msg").expect("read vendored Second Life message template");
    let template = build::template::parse("build/message_template.msg", &source).unwrap_or_else(|error| panic!("{error}"));
    let generated = build::template::generate(&template);
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR")).join("schema.rs");

    fs::write(output, generated).expect("write generated schema metadata");
}
