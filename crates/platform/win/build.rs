//! Windows (MSVC): the Media Foundation DLLs are delay-loaded, so the app also starts on
//! Windows "N" editions, which don't have them; the MF engine checks they exist before
//! it calls into them (`engines::os::mediafoundation`). The exe also gets the Swirvo
//! icon and version info.

fn main() {
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if os == "windows" && env == "msvc" {
        for dll in ["mf.dll", "mfplat.dll", "mfreadwrite.dll"] {
            println!("cargo:rustc-link-arg=/DELAYLOAD:{dll}");
        }
        println!("cargo:rustc-link-arg=delayimp.lib");
        // The app icon and version info (packaging/icon/swirvo.res, from
        // scripts/build-res.sh); lld-link takes a compiled .res as an input.
        let manifest =
            std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default());
        if let Some(res) = manifest
            .ancestors()
            .map(|d| d.join("packaging/icon/swirvo.res"))
            .find(|p| p.is_file())
        {
            println!("cargo:rustc-link-arg={}", res.display());
            println!("cargo:rerun-if-changed={}", res.display());
        }
    }
    println!("cargo:rerun-if-changed=build.rs");
}
