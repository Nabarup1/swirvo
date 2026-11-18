//! Password protection (AES-256, the PDF 2.0 standard security handler), unlocking, and
//! respecting the permissions of protected files.
//!
//! Rules (docs/17 §4):
//! - **Unlock needs the right password.** Removing printing/copying/editing restrictions
//!   needs the *owner* password; the open password only removes protection from a file
//!   that has no restrictions. We never strip someone else's restrictions without it.
//! - **Edits keep protection.** A protected file edited in place (stamps, crop, compress)
//!   is saved with the same passwords and restrictions ([`super::save`] re-applies them).
//! - **Permissions are honoured.** A tool that needs a permission the file withholds
//!   (copying text out, assembling pages, editing) refuses with a clear message.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use lopdf::encryption::crypt_filters::{Aes256CryptFilter, CryptFilter};
use lopdf::{Document, EncryptionState, EncryptionVersion, Object, Permissions};
use swirvo_core::EngineError;

/// What someone who opens the file with the *open* password may do. The owner password
/// (needed to change these) always allows everything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Allow {
    pub print: bool,
    pub copy: bool,
    pub edit: bool,
}

impl Default for Allow {
    fn default() -> Self {
        Allow {
            print: true,
            copy: true,
            edit: true,
        }
    }
}

fn permissions(a: Allow) -> Permissions {
    let mut p = Permissions::COPYABLE_FOR_ACCESSIBILITY;
    if a.print {
        p |= Permissions::PRINTABLE | Permissions::PRINTABLE_IN_HIGH_QUALITY;
    }
    if a.copy {
        p |= Permissions::COPYABLE;
    }
    if a.edit {
        p |= Permissions::MODIFIABLE
            | Permissions::ANNOTABLE
            | Permissions::FILLABLE
            | Permissions::ASSEMBLABLE;
    }
    p
}

const EVERYTHING: Permissions = Permissions::PRINTABLE
    .union(Permissions::PRINTABLE_IN_HIGH_QUALITY)
    .union(Permissions::COPYABLE)
    .union(Permissions::MODIFIABLE)
    .union(Permissions::ANNOTABLE)
    .union(Permissions::FILLABLE)
    .union(Permissions::ASSEMBLABLE);

fn random_hex(n: usize) -> Result<String, EngineError> {
    let mut b = vec![0u8; n];
    getrandom::fill(&mut b).map_err(|e| EngineError::Encode(format!("random: {e}")))?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// Encrypt with AES-256. `open_password` is needed to open the file (empty: anyone can
/// open it, but `allow` still applies); `owner_password` (random when None, so nobody can
/// lift the restrictions) is needed to change them.
pub fn protect(
    input: &Path,
    out: &Path,
    open_password: &str,
    owner_password: Option<&str>,
    allow: Allow,
) -> Result<(), EngineError> {
    if open_password.is_empty() && allow == Allow::default() {
        return Err(EngineError::Invalid(
            "type a password, or restrict printing, copying or editing".into(),
        ));
    }
    if open_password.chars().count() > 127 {
        return Err(EngineError::Invalid(
            "that password is too long (127 characters at most)".into(),
        ));
    }
    let mut doc = super::open(input, None)?;
    if doc.was_encrypted() {
        return Err(EngineError::Invalid(
            "this PDF is already protected; unlock it first".into(),
        ));
    }
    let owner = match owner_password {
        Some(o) if !o.is_empty() => o.to_string(),
        _ => random_hex(24)?,
    };
    let mut key = [0u8; 32];
    getrandom::fill(&mut key).map_err(|e| EngineError::Encode(format!("random: {e}")))?;
    let filter: Arc<dyn CryptFilter> = Arc::new(Aes256CryptFilter);
    let version = EncryptionVersion::V5 {
        encrypt_metadata: true,
        crypt_filters: BTreeMap::from([(b"StdCF".to_vec(), filter)]),
        file_encryption_key: &key,
        stream_filter: b"StdCF".to_vec(),
        string_filter: b"StdCF".to_vec(),
        owner_password: &owner,
        user_password: open_password,
        permissions: permissions(allow),
    };
    let state = EncryptionState::try_from(version).map_err(EngineError::encode)?;
    // The AES-256 handler is read by every current viewer (Acrobat X and later, 2010).
    if doc.version.as_str() < "1.7" {
        doc.version = "1.7".into();
    }
    doc.encrypt(&state).map_err(EngineError::encode)?;
    super::save(&mut doc, out, false)
}

/// Does `password` authenticate as the owner of the (already decrypted) `doc`?
fn owner_ok(doc: &Document, password: &str) -> bool {
    let Some(state) = &doc.encryption_state else {
        return false;
    };
    let Ok(dict) = state.encode() else {
        return false;
    };
    let mut probe = Document::new();
    let id = probe.add_object(Object::Dictionary(dict));
    probe.trailer.set("Encrypt", Object::Reference(id));
    if let Ok(file_id) = doc.trailer.get(b"ID") {
        probe.trailer.set("ID", file_id.clone());
    }
    probe.authenticate_owner_password(password).is_ok()
}

fn restricted(doc: &Document) -> bool {
    doc.encryption_state
        .as_ref()
        .is_some_and(|s| !s.permissions().contains(EVERYTHING))
}

/// Remove the protection. Needs the owner password when the file restricts anything;
/// otherwise the open password.
pub fn unlock(input: &Path, out: &Path, password: &str) -> Result<(), EngineError> {
    let bytes = std::fs::read(input)?;
    let mut doc = match Document::load_mem(&bytes) {
        Ok(d) if !d.was_encrypted() && !d.is_encrypted() => {
            return Err(EngineError::Invalid(
                "this PDF isn't password-protected".into(),
            ));
        }
        // Opened without a password: it only restricts what readers may do.
        Ok(d) if !d.is_encrypted() => d,
        _ => super::open_bytes(&bytes, Some(password))?,
    };
    if restricted(&doc) && !owner_ok(&doc, password) {
        return Err(EngineError::Invalid(
            "this PDF limits printing, copying or editing; removing that needs its owner \
             (permissions) password"
                .into(),
        ));
    }
    doc.encryption_state = None;
    doc.trailer.remove(b"Encrypt");
    super::save(&mut doc, out, false)
}

/// Whether a PDF needs a password to open.
pub fn needs_password(input: &Path) -> Result<bool, EngineError> {
    let bytes = std::fs::read(input)?;
    Ok(match Document::load_mem(&bytes) {
        Ok(d) => d.is_encrypted(),
        Err(lopdf::Error::Decryption(_) | lopdf::Error::InvalidPassword) => true,
        Err(e) => return Err(EngineError::Decode(format!("couldn't open this PDF ({e})"))),
    })
}

/// What a tool needs the file to permit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    /// Edits in place that keep the protection (stamps, crop, compress).
    Edit,
    /// Rearranging pages in place (rotate).
    Assemble,
    /// Taking content out: text, images, page pictures, conversion to other formats.
    Copy,
    /// Writing a new file that can't carry the protection over (merge, split, N-up):
    /// only allowed when the file restricts nothing.
    Everything,
}

/// Fail with a clear message if `input` withholds what a tool needs. Unprotected files
/// pass; files that need a password to open fail (unlock them first).
pub fn permits(input: &Path, need: Need) -> Result<(), EngineError> {
    let bytes = std::fs::read(input)?;
    let doc = match Document::load_mem(&bytes) {
        Ok(d) if d.is_encrypted() => {
            return Err(EngineError::Invalid(
                "this PDF is password-protected; unlock it first".into(),
            ));
        }
        Ok(d) => d,
        Err(lopdf::Error::Decryption(_) | lopdf::Error::InvalidPassword) => {
            return Err(EngineError::Invalid(
                "this PDF is password-protected; unlock it first".into(),
            ));
        }
        // Damaged files are the tools' business (they may repair them).
        Err(_) => return Ok(()),
    };
    let Some(state) = &doc.encryption_state else {
        return Ok(());
    };
    let p = state.permissions();
    let (ok, what) = match need {
        Need::Edit => (p.contains(Permissions::MODIFIABLE), "editing"),
        Need::Assemble => (
            p.contains(Permissions::ASSEMBLABLE) || p.contains(Permissions::MODIFIABLE),
            "rearranging pages",
        ),
        Need::Copy => (p.contains(Permissions::COPYABLE), "copying its content"),
        Need::Everything => (p.contains(EVERYTHING), "changes like this"),
    };
    if ok {
        Ok(())
    } else {
        Err(EngineError::Invalid(format!(
            "this PDF's owner doesn't allow {what}; unlock it with the owner password first"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdfedit::stamp;
    use swirvo_core::CancelToken;

    #[test]
    fn protect_then_unlock() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("a.pdf");
        crate::pdfedit::testing::sample(&src, 2, false);
        let locked = d.path().join("locked.pdf");
        protect(&src, &locked, "s3cret pass", None, Allow::default()).unwrap();
        assert!(needs_password(&locked).unwrap());
        assert!(super::super::open(&locked, None).is_err());
        assert!(matches!(
            super::super::open(&locked, Some("wrong")),
            Err(EngineError::Invalid(_))
        ));
        assert!(
            protect(
                &locked,
                &d.path().join("x.pdf"),
                "a",
                None,
                Allow::default()
            )
            .is_err()
        );
        let open = d.path().join("open.pdf");
        assert!(unlock(&locked, &open, "nope").is_err());
        // No restrictions, so the open password is enough.
        unlock(&locked, &open, "s3cret pass").unwrap();
        assert!(!needs_password(&open).unwrap());
        let r = crate::pdf_tools::PageRenderer::open(&open).unwrap();
        assert_eq!(r.count(), 2);
        let img = r.session(|s| s.render_dpi(0, 36.0)).unwrap();
        assert!(img.pixels().any(|p| p[0] < 50));
        assert!(unlock(&src, &d.path().join("y.pdf"), "").is_err());
    }

    #[test]
    fn restrictions_need_the_owner_password_and_survive_edits() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("a.pdf");
        crate::pdfedit::testing::sample(&src, 1, false);
        let ro = d.path().join("ro.pdf");
        let allow = Allow {
            print: true,
            copy: false,
            edit: true,
        };
        protect(&src, &ro, "", Some("owner-pw"), allow).unwrap();
        // Anyone can open it…
        assert!(!needs_password(&ro).unwrap());
        // …but its restrictions only come off with the owner password.
        let free = d.path().join("free.pdf");
        assert!(unlock(&ro, &free, "").is_err());
        assert!(unlock(&ro, &free, "guess").is_err());
        unlock(&ro, &free, "owner-pw").unwrap();
        assert!(permits(&free, Need::Copy).is_ok());

        // Permissions are honoured…
        assert!(permits(&ro, Need::Copy).is_err());
        assert!(permits(&ro, Need::Everything).is_err());
        assert!(permits(&ro, Need::Edit).is_ok());
        // …and an edit keeps the protection, with the same owner password.
        let stamped = d.path().join("stamped.pdf");
        stamp::stamp(&ro, &stamped, None, &CancelToken::new(), |p| {
            vec![stamp::Item::Rect {
                rect: [0.0, 0.0, p.w / 4.0, p.h / 4.0],
                color: [255, 0, 0],
                opacity: 1.0,
            }]
        })
        .unwrap();
        assert!(permits(&stamped, Need::Copy).is_err());
        let again = d.path().join("again.pdf");
        assert!(unlock(&stamped, &again, "").is_err());
        unlock(&stamped, &again, "owner-pw").unwrap();
        let r = crate::pdf_tools::PageRenderer::open(&again).unwrap();
        let img = r.session(|s| s.render_dpi(0, 36.0)).unwrap();
        assert!(
            img.pixels().any(|p| p[0] > 200 && p[1] < 60),
            "the stamp is there"
        );
    }
}
