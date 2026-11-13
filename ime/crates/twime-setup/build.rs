include!("../../build/versioninfo.rs");

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        let rc = versioninfo_rc("地瓜輸入法 安裝工具", "twime-setup.exe", false);
        embed_resource::compile(rc, embed_resource::NONE).manifest_optional().unwrap();
    }
}
