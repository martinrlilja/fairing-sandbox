fn main() -> Result<(), Box<dyn std::error::Error>> {
    const PROTO_FILES: &[&str] = &[
        "../proto/sites/v1beta1/sites.proto",
        "../proto/teams/v1beta1/teams.proto",
        "../proto/users/v1beta1/users.proto",
    ];

    tonic_build::configure().compile(PROTO_FILES, &["../proto"])?;

    for proto_file in PROTO_FILES {
        println!("cargo:rerun-if-changed={}", proto_file);
    }

    Ok(())
}
