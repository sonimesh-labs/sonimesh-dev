//! Generates Rust control-plane schema bindings from the canonical Protobuf source.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=../../proto/control.proto");

    let protoc = protoc_bin_vendored::protoc_bin_path()?;
    let mut config = prost_build::Config::new();
    config.protoc_executable(protoc);
    config.compile_protos(&["../../proto/control.proto"], &["../../proto"])?;
    Ok(())
}
