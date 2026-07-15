fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_client(true)
        .build_server(true)
        .compile_protos(&["../../proto/worker.proto"], &["../../proto"])?;
    println!("cargo:rerun-if-changed=../../proto/worker.proto");
    Ok(())
}
