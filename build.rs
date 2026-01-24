fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Only compile proto if grpc feature is enabled
    #[cfg(feature = "grpc")]
    {
        let proto_file = "proto/exchange.proto";
        let out_dir = "src/grpc/generated";

        // Create output directory if it doesn't exist
        std::fs::create_dir_all(out_dir)?;

        tonic_build::configure()
            .build_server(true)
            .build_client(true)
            .out_dir(out_dir)
            .compile_protos(&[proto_file], &["proto"])?;

        println!("cargo:rerun-if-changed={}", proto_file);
    }

    Ok(())
}
