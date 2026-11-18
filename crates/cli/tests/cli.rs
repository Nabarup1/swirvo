//! End-to-end: drive the real `swirvo` binary against files in a temp folder.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn swirvo(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_swirvo"))
        .args(args)
        .current_dir(dir)
        // Keep frecency writes inside the test folder.
        .env("SWIRVO_DATA_DIR", dir.join(".data"))
        .output()
        .expect("run swirvo")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn png(path: &Path) {
    let img = image::RgbaImage::from_fn(32, 24, |x, y| {
        image::Rgba([x as u8 * 8, y as u8 * 10, 128, 255])
    });
    img.save_with_format(path, image::ImageFormat::Png)
        .expect("write png");
}

#[test]
fn convert_never_overwrites_and_keeps_the_original() {
    let d = tempfile::tempdir().unwrap();
    png(&d.path().join("shot.png"));
    let before = fs::read(d.path().join("shot.png")).unwrap();
    fs::write(d.path().join("shot.jpg"), b"user's own file").unwrap();

    let o = swirvo(d.path(), &["convert", "shot.png", "--to", "jpg", "--json"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let line = stdout(&o);
    let ev: serde_json::Value = serde_json::from_str(line.lines().next().unwrap()).unwrap();
    assert_eq!(ev["event"], "done");
    assert!(ev["output"].as_str().unwrap().ends_with("shot (2).jpg"));

    assert_eq!(
        fs::read(d.path().join("shot.jpg")).unwrap(),
        b"user's own file"
    );
    assert_eq!(fs::read(d.path().join("shot.png")).unwrap(), before);
    let out = image::open(d.path().join("shot (2).jpg")).unwrap();
    assert_eq!((out.width(), out.height()), (32, 24));
    // No temp files left behind.
    let leftovers: Vec<_> = fs::read_dir(d.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with('.') && n != ".data")
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn data_round_trip_and_detection() {
    let d = tempfile::tempdir().unwrap();
    fs::write(
        d.path().join("t.csv"),
        "name,qty,zip\nfig,3,007\npear,10,123\n",
    )
    .unwrap();
    let o = swirvo(d.path(), &["convert", "t.csv", "--to", "json"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let v: serde_json::Value =
        serde_json::from_slice(&fs::read(d.path().join("t.json")).unwrap()).unwrap();
    assert_eq!(v[0]["qty"], 3);
    assert_eq!(v[0]["zip"], "007", "leading zeros stay text");

    // A PNG wearing a .jpg extension: content wins and the mismatch is reported.
    png(&d.path().join("liar.jpg"));
    let o = swirvo(d.path(), &["detect", "liar.jpg", "--json"]);
    let v: serde_json::Value = serde_json::from_str(stdout(&o).lines().next().unwrap()).unwrap();
    assert_eq!(v["format"], "png");
    assert_eq!(v["mismatch"], true);
}

#[test]
fn extract_makes_a_new_folder() {
    let d = tempfile::tempdir().unwrap();
    fs::create_dir(d.path().join("src")).unwrap();
    fs::write(d.path().join("src").join("a.txt"), b"swirvo sample").unwrap();
    {
        use std::io::Write;
        let mut z = zip::ZipWriter::new(fs::File::create(d.path().join("Assets.zip")).unwrap());
        z.start_file("a.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        z.write_all(b"swirvo sample").unwrap();
        z.finish().unwrap();
    }
    fs::create_dir(d.path().join("Assets")).unwrap();
    let o = swirvo(d.path(), &["extract", "Assets.zip"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(
        fs::read(d.path().join("Assets (2)").join("a.txt")).unwrap(),
        b"swirvo sample"
    );
    assert!(
        fs::read_dir(d.path().join("Assets"))
            .unwrap()
            .next()
            .is_none(),
        "existing folder untouched"
    );

    let o = swirvo(d.path(), &["extract", "src/a.txt"]);
    assert!(!o.status.success());
}

#[test]
fn plan_and_formats() {
    let d = tempfile::tempdir().unwrap();
    let o = swirvo(d.path(), &["plan", "png", "--to", "pdf"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(stdout(&o).contains("png → pdf"), "{}", stdout(&o));
    let o = swirvo(d.path(), &["formats", "--from", "md"]);
    assert!(stdout(&o).contains("html"));
    let o = swirvo(d.path(), &["convert", "missing.png", "--to", "jpg"]);
    assert!(!o.status.success());
}

#[test]
fn page_tools_take_a_selection() {
    let d = tempfile::tempdir().unwrap();
    for n in ["a.png", "b.png", "c.png"] {
        png(&d.path().join(n));
    }
    let o = swirvo(
        d.path(),
        &["tool", "images-to-pdf", "a.png", "b.png", "c.png"],
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let o = swirvo(
        d.path(),
        &[
            "tool",
            "pdf-pages",
            "a (combined).pdf",
            "--pages",
            "1, 3",
            "--json",
        ],
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(d.path().join("a (combined) (pages 1, 3).pdf").exists());
    let o = swirvo(
        d.path(),
        &[
            "tool",
            "pdf-to-jpg",
            "a (combined).pdf",
            "--pages",
            "2",
            "--dpi",
            "72",
        ],
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(d.path().join("a (combined) p02.jpg").exists());
    let o = swirvo(
        d.path(),
        &["tool", "pdf-to-jpg", "a (combined).pdf", "--pages", "9"],
    );
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("doesn't exist"));
}
