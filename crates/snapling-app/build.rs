fn main() {
    println!("cargo:rerun-if-env-changed=SNAPLING_COMMIT");
    println!(
        "cargo:rustc-env=SNAPLING_TARGET={}",
        std::env::var("TARGET").unwrap_or_else(|_| "unknown-target".into())
    );
    println!(
        "cargo:rustc-env=SNAPLING_COMMIT={}",
        std::env::var("SNAPLING_COMMIT").unwrap_or_else(|_| "devel".into())
    );
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=../../assets/brand/snapling.ico");
        winresource::WindowsResource::new()
            .set_icon("../../assets/brand/snapling.ico")
            .compile()
            .expect("exe resources");
    }
}
