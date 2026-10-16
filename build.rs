fn main() {
    println!("cargo:rerun-if-changed=Resources/Icon/icon_256.png");
    println!("cargo:rerun-if-changed=build.rs");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_icon();
    }
}

#[cfg(windows)]
fn embed_icon() {
    let png = std::fs::read("Resources/Icon/icon_256.png").expect("Resources/Icon/icon_256.png");
    const HEADER_LEN: u32 = 6 + 16;
    let mut ico: Vec<u8> = vec![0, 0, 1, 0, 1, 0];
    // One PNG entry: width and height 0 mean 256 px.
    ico.extend([0, 0, 0, 0, 1, 0, 32, 0]);
    ico.extend((png.len() as u32).to_le_bytes());
    ico.extend(HEADER_LEN.to_le_bytes());
    ico.extend(png);
    let path = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("icon.ico");
    std::fs::write(&path, ico).expect("write icon.ico");
    let mut resource = winresource::WindowsResource::new();
    resource.set_icon(path.to_str().unwrap());
    resource.set("ProductName", "Agents Pixel Dungeon");
    resource.set("FileDescription", "Agents Pixel Dungeon");
    if let Err(e) = resource.compile() {
        println!("cargo:warning=could not embed the Windows icon: {e}");
    }
}
