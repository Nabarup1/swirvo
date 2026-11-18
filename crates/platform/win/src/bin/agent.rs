//! `swirvo-agent`: the background overlay process (Windows). A GUI-subsystem app: no
//! console window when double-clicked; it attaches to the parent console when run from one.

#![windows_subsystem = "windows"]

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    swirvo_platform_win::run()
}

#[cfg(not(windows))]
fn main() -> std::process::ExitCode {
    eprintln!("swirvo-agent is the Windows overlay. On Linux, use the `swirvo` CLI for now.");
    std::process::ExitCode::from(2)
}
