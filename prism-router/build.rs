fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .out_dir("src/gen")
        .compile_protos(&["../proto/prism/router/v1/router.proto"], &["../proto"])?;
    Ok(())
}
