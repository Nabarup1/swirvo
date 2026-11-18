//! Editing a Windows `PATH` value (`dir1;dir2;…`) without disturbing anything else in it.
//! Portable and tested everywhere; `install.rs` reads and writes the registry around it.

/// Windows compares folders case-insensitively and ignores a trailing backslash.
fn same(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.trim().trim_end_matches(['\\', '/']).to_lowercase();
    norm(a) == norm(b)
}

/// `list` with `dir` appended, or `None` when it's already there.
pub fn with(list: &str, dir: &str) -> Option<String> {
    if list.split(';').any(|e| same(e, dir)) {
        return None;
    }
    let base = list.trim_end_matches(';');
    Some(if base.trim().is_empty() {
        dir.to_string()
    } else {
        format!("{base};{dir}")
    })
}

/// `list` without any entry equal to `dir`, or `None` when it isn't there. Every other
/// entry, including empty ones and their order, is kept exactly.
pub fn without(list: &str, dir: &str) -> Option<String> {
    if !list.split(';').any(|e| same(e, dir)) {
        return None;
    }
    Some(
        list.split(';')
            .filter(|e| !same(e, dir))
            .collect::<Vec<_>>()
            .join(";"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const BIN: &str = r"C:\Users\a\AppData\Local\Programs\Swirvo\bin";

    #[test]
    fn adds_once() {
        assert_eq!(with("", BIN).as_deref(), Some(BIN));
        assert_eq!(
            with(r"C:\x;%USERPROFILE%\y", BIN).as_deref(),
            Some(&*format!(r"C:\x;%USERPROFILE%\y;{BIN}"))
        );
        assert_eq!(
            with(r"C:\x;", BIN).as_deref(),
            Some(&*format!(r"C:\x;{BIN}"))
        );
        // Already there, in another case or with a trailing backslash.
        assert_eq!(with(&format!(r"C:\x;{}\", BIN.to_uppercase()), BIN), None);
    }

    #[test]
    fn removes_only_ours() {
        let list = format!(r"C:\x;;{BIN};%USERPROFILE%\y");
        assert_eq!(
            without(&list, BIN).as_deref(),
            Some(r"C:\x;;%USERPROFILE%\y")
        );
        assert_eq!(without(r"C:\x;C:\y", BIN), None);
        assert_eq!(without(BIN, BIN).as_deref(), Some(""));
        // Round trip.
        let base = r"C:\x;D:\tools";
        assert_eq!(
            without(&with(base, BIN).unwrap(), BIN).as_deref(),
            Some(base)
        );
    }
}
