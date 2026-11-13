include!("../../build/versioninfo.rs");

fn main() {
    println!("cargo:rerun-if-changed=res/twime-icons.res");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        let path = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
            .join("res/twime-icons.res");
        println!("cargo:rustc-link-arg={}", path.display());
        // The DLL is a cdylib: link the version resource into every artifact, not only binaries.
        let rc = versioninfo_rc("地瓜輸入法 TSF 文字服務", "twime_tsf.dll", true);
        embed_resource::compile_for_everything(rc, embed_resource::NONE).manifest_optional().unwrap();
    }
}
