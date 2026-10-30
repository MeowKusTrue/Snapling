fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=../../assets/brand/snapling.ico");
        winresource::WindowsResource::new()
            .set_icon("../../assets/brand/snapling.ico")
            .compile()
            .expect("exe resources");
    }
}
