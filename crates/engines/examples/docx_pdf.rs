//! DOCX → PDF with the built-in layout engine, for the quality loop (docs/16 §6).
//!
//! `cargo run --release -p swirvo-engines --example docx_pdf -- in.docx out.pdf`

use std::time::Instant;

fn main() {
    let args: Vec<std::path::PathBuf> = std::env::args_os().skip(1).map(Into::into).collect();
    if args.len() < 2 {
        eprintln!("usage: docx_pdf IN.docx OUT.pdf [IN2.docx OUT2.pdf …]");
        std::process::exit(2);
    }
    for pair in args.chunks(2) {
        let [input, output] = pair else { break };
        let bytes = match std::fs::read(input) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("{}: {e}", input.display());
                continue;
            }
        };
        let t = Instant::now();
        let title = input
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        match swirvo_engines::docxlayout::convert(
            &bytes,
            &title,
            output,
            &swirvo_core::CancelToken::new(),
        ) {
            Ok(n) => println!(
                "{} → {}: {n} pages in {} ms",
                input.display(),
                output.display(),
                t.elapsed().as_millis()
            ),
            Err(e) => eprintln!("{}: {e}", input.display()),
        }
    }
}
