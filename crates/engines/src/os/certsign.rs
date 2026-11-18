//! Certificate signatures through the operating system (docs/17 §2), an OS engine: the
//! user's own certificates in the Windows certificate store ("Personal"), CryptoAPI to
//! sign and verify CMS (PKCS#7) detached signatures, and Windows' chain engine to say
//! whether a signer is trusted. No crypto code or crates of our own; keys never leave
//! the store (a smart card or a protected key asks for its PIN in Windows' own dialog).
//!
//! The PDF side (signature field, byte ranges, incremental update) is
//! [`crate::pdfedit::sign`]. Other systems report signing as unavailable.

use std::path::Path;

use swirvo_core::{CancelToken, EngineError};

use crate::pdfedit::sign::{self, Found, Setup, Visible};

/// A certificate the user can sign with (it has a private key).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cert {
    pub subject: String,
    pub issuer: String,
    /// "YYYY-MM-DD".
    pub expires: String,
    /// SHA-1 thumbprint, upper-case hex, as Windows shows it.
    pub thumbprint: String,
    /// Inside its validity period now.
    pub current: bool,
}

/// What checking one signature found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    /// From the signer's certificate.
    pub signer: Option<String>,
    /// The signing time inside the signature (signed, so more trustworthy than `/M`).
    pub signed_at: Option<String>,
    /// The signed bytes match the signature.
    pub intact: bool,
    /// The certificate chains to a root this PC trusts, valid at signing time.
    pub trusted: bool,
    /// Why it isn't trusted, in words.
    pub trust_note: Option<String>,
}

pub fn available() -> bool {
    cfg!(windows)
}

/// The user's certificates with a private key, newest expiry first.
pub fn certificates() -> Result<Vec<Cert>, EngineError> {
    let mut v = imp::certificates()?;
    v.sort_by(|a, b| b.current.cmp(&a.current).then(b.expires.cmp(&a.expires)));
    Ok(v)
}

/// Pick a certificate: one whose name or thumbprint contains `query` (any case), else
/// the only current one.
pub fn choose(certs: &[Cert], query: Option<&str>) -> Result<Cert, EngineError> {
    let current: Vec<&Cert> = certs.iter().filter(|c| c.current).collect();
    let pool: Vec<&Cert> = match query.map(str::trim).filter(|q| !q.is_empty()) {
        Some(q) => {
            let q = q.to_lowercase();
            let compact = q.replace([' ', ':'], "");
            let hits: Vec<&Cert> = certs
                .iter()
                .filter(|c| {
                    c.subject.to_lowercase().contains(&q)
                        || c.thumbprint.to_lowercase().starts_with(&compact)
                })
                .collect();
            if hits.is_empty() {
                return Err(EngineError::Invalid(format!(
                    "no certificate matches \u{201c}{q}\u{201d}; you have: {}",
                    names(certs)
                )));
            }
            hits
        }
        None => current.clone(),
    };
    match pool.as_slice() {
        [] if certs.is_empty() => Err(EngineError::Invalid(
            "there's no certificate with a private key in your Personal certificate store".into(),
        )),
        [] => Err(EngineError::Invalid(format!(
            "all your certificates have expired: {}",
            names(certs)
        ))),
        [one] => Ok((*one).clone()),
        many => {
            let now: Vec<&&Cert> = many.iter().filter(|c| c.current).collect();
            match now.as_slice() {
                [one] => Ok((**one).clone()),
                _ => Err(EngineError::Invalid(format!(
                    "say which certificate to use: {}",
                    names(&many.iter().map(|c| (*c).clone()).collect::<Vec<_>>())
                ))),
            }
        }
    }
}

fn names(certs: &[Cert]) -> String {
    let v: Vec<String> = certs
        .iter()
        .take(6)
        .map(|c| format!("{} (expires {})", c.subject, c.expires))
        .collect();
    if v.is_empty() {
        "none".into()
    } else {
        v.join(", ")
    }
}

/// Options for [`sign_pdf`].
#[derive(Debug, Clone, Default)]
pub struct SignOptions {
    /// Part of the certificate's name, or its thumbprint; None picks the only one.
    pub certificate: Option<String>,
    pub reason: Option<String>,
    pub location: Option<String>,
    /// A visible signature box; None signs invisibly.
    pub visible: Option<Visible>,
}

/// Sign `input` into `out`. Returns the certificate used.
pub fn sign_pdf(
    input: &Path,
    out: &Path,
    opts: &SignOptions,
    cancel: &CancelToken,
) -> Result<Cert, EngineError> {
    if !available() {
        return Err(unavailable());
    }
    let bytes = std::fs::read(input)?;
    let cert = choose(&certificates()?, opts.certificate.as_deref())?;
    let setup = Setup {
        name: cert.subject.clone(),
        reason: opts.reason.clone(),
        location: opts.location.clone(),
        time: sign::now(),
        visible: opts.visible.clone(),
    };
    let prepared = sign::prepare(&bytes, &setup)?;
    cancel.check()?;
    let der = imp::sign(&prepared.signed_data(), &cert.thumbprint)?;
    std::fs::write(out, prepared.finish(&der)?)?;
    Ok(cert)
}

/// Every signature in `input`, with what checking it found (None where checking isn't
/// possible on this system).
pub fn verify_pdf(input: &Path) -> Result<Vec<(Found, Option<Verdict>)>, EngineError> {
    let bytes = std::fs::read(input)?;
    let found = sign::signatures(&bytes)?;
    Ok(found
        .into_iter()
        .map(|f| {
            let v = (available() && f.range_ok && !f.der.is_empty())
                .then(|| imp::verify(&f.data, &f.der).ok())
                .flatten();
            (f, v)
        })
        .collect())
}

/// One line per signature, for the clipboard and the result message.
pub fn report(found: &[(Found, Option<Verdict>)]) -> Vec<String> {
    found
        .iter()
        .map(|(f, v)| {
            let who = v
                .as_ref()
                .and_then(|v| v.signer.clone())
                .or_else(|| f.name.clone())
                .unwrap_or_else(|| "unknown signer".into());
            let when = v
                .as_ref()
                .and_then(|v| v.signed_at.clone())
                .or_else(|| f.time.as_deref().map(sign::readable_date))
                .map(|t| format!(" on {t}"))
                .unwrap_or_default();
            let state = match v {
                _ if !f.range_ok => "the signature doesn't cover the document properly".to_string(),
                None if available() => "the signature can't be read".to_string(),
                None => "not checked (needs Windows)".to_string(),
                Some(v) if !v.intact => "INVALID: the signed content was changed".to_string(),
                Some(v) => {
                    let mut s = String::from("intact");
                    if !f.whole_file {
                        s.push_str(", but the file was changed after this signature");
                    }
                    match (&v.trusted, &v.trust_note) {
                        (true, _) => s.push_str("; signer trusted"),
                        (false, Some(n)) => s.push_str(&format!("; signer not trusted ({n})")),
                        (false, None) => s.push_str("; signer not trusted"),
                    }
                    s
                }
            };
            let reason = f
                .reason
                .as_ref()
                .map(|r| format!(" · reason: {r}"))
                .unwrap_or_default();
            format!("{}: {who}{when} — {state}{reason}", f.field)
        })
        .collect()
}

fn unavailable() -> EngineError {
    EngineError::Invalid(
        "signing with a certificate uses the Windows certificate store, which this system \
         doesn't have"
            .into(),
    )
}

/// FILETIME ticks (100 ns since 1601) for a Unix time, and back to a readable date.
#[cfg_attr(not(windows), allow(dead_code))]
fn unix_to_filetime(secs: u64, nanos: u32) -> u64 {
    (secs + 11_644_473_600) * 10_000_000 + u64::from(nanos / 100)
}

#[cfg_attr(not(windows), allow(dead_code))]
fn filetime_text(ticks: u64, with_time: bool) -> String {
    let secs = (ticks / 10_000_000).saturating_sub(11_644_473_600);
    let (y, m, d) = crate::archive::civil_from_days((secs / 86_400) as i64);
    if with_time {
        let r = secs % 86_400;
        format!(
            "{y:04}-{m:02}-{d:02} {:02}:{:02} UTC",
            r / 3600,
            r / 60 % 60
        )
    } else {
        format!("{y:04}-{m:02}-{d:02}")
    }
}

#[cfg(not(windows))]
mod imp {
    use super::{Cert, Verdict, unavailable};
    use swirvo_core::EngineError;

    pub fn certificates() -> Result<Vec<Cert>, EngineError> {
        Err(unavailable())
    }
    pub fn sign(_data: &[u8], _thumbprint: &str) -> Result<Vec<u8>, EngineError> {
        Err(unavailable())
    }
    pub fn verify(_data: &[u8], _der: &[u8]) -> Result<Verdict, EngineError> {
        Err(unavailable())
    }
}

#[cfg(windows)]
mod imp {
    use super::{Cert, Verdict, filetime_text, unix_to_filetime};
    use swirvo_core::EngineError;
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::Security::Cryptography::*;
    use windows::core::{PSTR, w};

    const ENC: u32 = X509_ASN_ENCODING.0 | PKCS_7_ASN_ENCODING.0;

    fn os(what: &'static str) -> impl Fn(windows::core::Error) -> EngineError {
        move |e| EngineError::Encode(format!("{what}: {}", e.message()))
    }

    /// An open certificate store, closed on drop.
    struct Store(HCERTSTORE);

    impl Drop for Store {
        fn drop(&mut self) {
            // SAFETY: the handle came from CertOpen*Store and is closed once.
            unsafe {
                let _ = CertCloseStore(Some(self.0), 0);
            }
        }
    }

    fn personal() -> Result<Store, EngineError> {
        // SAFETY: plain FFI call with a static wide string.
        unsafe { CertOpenSystemStoreW(None, w!("MY")) }
            .map(Store)
            .map_err(os("opening your certificate store"))
    }

    /// # Safety
    /// `ctx` must be a valid certificate context.
    unsafe fn name(ctx: *const CERT_CONTEXT, issuer: bool) -> String {
        let flag = if issuer { CERT_NAME_ISSUER_FLAG } else { 0 };
        // SAFETY: valid context (caller); the buffer is sized by the first call.
        unsafe {
            let n = CertGetNameStringW(ctx, CERT_NAME_SIMPLE_DISPLAY_TYPE, flag, None, None);
            let mut buf = vec![0u16; n.max(1) as usize];
            CertGetNameStringW(
                ctx,
                CERT_NAME_SIMPLE_DISPLAY_TYPE,
                flag,
                None,
                Some(&mut buf),
            );
            let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            String::from_utf16_lossy(&buf[..end])
        }
    }

    /// # Safety
    /// `ctx` must be a valid certificate context.
    unsafe fn property(ctx: *const CERT_CONTEXT, id: u32) -> Option<Vec<u8>> {
        let mut len = 0u32;
        // SAFETY: size query, then a buffer of that size.
        unsafe {
            CertGetCertificateContextProperty(ctx, id, None, &mut len).ok()?;
            let mut buf = vec![0u8; len as usize];
            CertGetCertificateContextProperty(ctx, id, Some(buf.as_mut_ptr().cast()), &mut len)
                .ok()?;
            buf.truncate(len as usize);
            Some(buf)
        }
    }

    /// # Safety
    /// `ctx` must be a valid certificate context.
    unsafe fn thumbprint(ctx: *const CERT_CONTEXT) -> String {
        // SAFETY: valid context (caller).
        unsafe { property(ctx, CERT_SHA1_HASH_PROP_ID) }
            .unwrap_or_default()
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect()
    }

    fn ticks(ft: FILETIME) -> u64 {
        (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime)
    }

    fn now_ticks() -> u64 {
        let d = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        unix_to_filetime(d.as_secs(), d.subsec_nanos())
    }

    fn filetime(t: u64) -> FILETIME {
        FILETIME {
            dwLowDateTime: t as u32,
            dwHighDateTime: (t >> 32) as u32,
        }
    }

    pub fn certificates() -> Result<Vec<Cert>, EngineError> {
        let store = personal()?;
        let now = now_ticks();
        let mut out = Vec::new();
        let mut prev: *mut CERT_CONTEXT = std::ptr::null_mut();
        loop {
            // SAFETY: enumeration frees the previous context it's given; the last one
            // (null) ends the loop, so nothing is left to free.
            let ctx =
                unsafe { CertEnumCertificatesInStore(store.0, (!prev.is_null()).then_some(prev)) };
            if ctx.is_null() {
                break;
            }
            prev = ctx;
            // SAFETY: ctx is a valid context from the store until the next call.
            unsafe {
                if property(ctx, CERT_KEY_PROV_INFO_PROP_ID).is_none()
                    && property(ctx, CERT_NCRYPT_KEY_HANDLE_PROP_ID).is_none()
                {
                    continue;
                }
                let info = &*(*ctx).pCertInfo;
                let (nb, na) = (ticks(info.NotBefore), ticks(info.NotAfter));
                out.push(Cert {
                    subject: name(ctx, false),
                    issuer: name(ctx, true),
                    expires: filetime_text(na, false),
                    thumbprint: thumbprint(ctx),
                    current: nb <= now && now <= na,
                });
            }
        }
        Ok(out)
    }

    /// The certificate with this thumbprint; the caller frees it.
    fn find(store: &Store, thumb: &str) -> Option<*mut CERT_CONTEXT> {
        let mut prev: *mut CERT_CONTEXT = std::ptr::null_mut();
        loop {
            // SAFETY: as in `certificates`; a match is returned without enumerating on,
            // so the caller owns it.
            let ctx =
                unsafe { CertEnumCertificatesInStore(store.0, (!prev.is_null()).then_some(prev)) };
            if ctx.is_null() {
                return None;
            }
            // SAFETY: valid context.
            if unsafe { thumbprint(ctx) }.eq_ignore_ascii_case(thumb) {
                return Some(ctx);
            }
            prev = ctx;
        }
    }

    pub fn sign(data: &[u8], thumb: &str) -> Result<Vec<u8>, EngineError> {
        let store = personal()?;
        let ctx = find(&store, thumb).ok_or_else(|| {
            EngineError::Invalid("that certificate is gone from the store".into())
        })?;
        // SAFETY: ctx is ours to free.
        let r = unsafe { sign_with(ctx, data) };
        // SAFETY: freed once, after use.
        unsafe {
            let _ = CertFreeCertificateContext(Some(ctx));
        }
        r
    }

    /// A detached CMS signature (SHA-256) over `data` by `ctx`'s private key, with the
    /// certificate chain and a signed signing-time attribute.
    ///
    /// # Safety
    /// `ctx` must be a valid certificate context with a private key.
    pub(super) unsafe fn sign_with(
        ctx: *const CERT_CONTEXT,
        data: &[u8],
    ) -> Result<Vec<u8>, EngineError> {
        // SAFETY: every pointer handed to CryptoAPI points into locals that outlive the
        // calls; output buffers are sized by the first call of each pair.
        unsafe {
            // The chain to include (the signer's certificate first).
            let mut chain: *mut CERT_CHAIN_CONTEXT = std::ptr::null_mut();
            let para = CERT_CHAIN_PARA {
                cbSize: std::mem::size_of::<CERT_CHAIN_PARA>() as u32,
                ..Default::default()
            };
            let mut certs: Vec<*mut CERT_CONTEXT> = Vec::new();
            if CertGetCertificateChain(None, ctx, None, None, &para, 0, None, &mut chain).is_ok()
                && !chain.is_null()
                && (*chain).cChain > 0
            {
                let simple = &**(*chain).rgpChain;
                for i in 0..simple.cElement as usize {
                    let el = &**simple.rgpElement.add(i);
                    certs.push(el.pCertContext as *mut CERT_CONTEXT);
                }
            }
            if certs.is_empty() {
                certs.push(ctx as *mut CERT_CONTEXT);
            }
            // Signing time, as a signed attribute.
            let now = filetime(now_ticks());
            let mut len = 0u32;
            CryptEncodeObjectEx(
                X509_ASN_ENCODING,
                PKCS_UTC_TIME,
                (&now as *const FILETIME).cast(),
                CRYPT_ENCODE_OBJECT_FLAGS(0),
                None,
                None,
                &mut len,
            )
            .map_err(os("encoding the signing time"))?;
            let mut time = vec![0u8; len as usize];
            CryptEncodeObjectEx(
                X509_ASN_ENCODING,
                PKCS_UTC_TIME,
                (&now as *const FILETIME).cast(),
                CRYPT_ENCODE_OBJECT_FLAGS(0),
                None,
                Some(time.as_mut_ptr().cast()),
                &mut len,
            )
            .map_err(os("encoding the signing time"))?;
            time.truncate(len as usize);
            let mut blob = CRYPT_INTEGER_BLOB {
                cbData: time.len() as u32,
                pbData: time.as_mut_ptr(),
            };
            let mut attr = CRYPT_ATTRIBUTE {
                pszObjId: PSTR(szOID_RSA_signingTime.0 as *mut u8),
                cValue: 1,
                rgValue: &mut blob,
            };
            let sp = CRYPT_SIGN_MESSAGE_PARA {
                cbSize: std::mem::size_of::<CRYPT_SIGN_MESSAGE_PARA>() as u32,
                dwMsgEncodingType: ENC,
                pSigningCert: ctx,
                HashAlgorithm: CRYPT_ALGORITHM_IDENTIFIER {
                    pszObjId: PSTR(szOID_NIST_sha256.0 as *mut u8),
                    Parameters: CRYPT_INTEGER_BLOB::default(),
                },
                cMsgCert: certs.len() as u32,
                rgpMsgCert: certs.as_mut_ptr(),
                cAuthAttr: 1,
                rgAuthAttr: &mut attr,
                ..Default::default()
            };
            let ptrs = [data.as_ptr()];
            let lens = [data.len() as u32];
            let mut size = 0u32;
            let r = CryptSignMessage(
                &sp,
                true,
                1,
                Some(ptrs.as_ptr()),
                lens.as_ptr(),
                None,
                &mut size,
            )
            .and_then(|_| {
                let mut out = vec![0u8; size as usize];
                CryptSignMessage(
                    &sp,
                    true,
                    1,
                    Some(ptrs.as_ptr()),
                    lens.as_ptr(),
                    Some(out.as_mut_ptr()),
                    &mut size,
                )
                .map(|_| {
                    out.truncate(size as usize);
                    out
                })
            });
            if !chain.is_null() {
                CertFreeCertificateChain(chain);
            }
            r.map_err(os("signing"))
        }
    }

    /// The signing time inside a CMS signature, as FILETIME ticks.
    fn signing_time(der: &[u8]) -> Option<u64> {
        // SAFETY: the message handle is closed on every path; buffers are sized by the
        // first call and kept 8-byte aligned for the attribute structures.
        unsafe {
            let msg = CryptMsgOpenToDecode(ENC, CMSG_DETACHED_FLAG, 0, None, None, None);
            if msg.is_null() {
                return None;
            }
            let found = (|| {
                CryptMsgUpdate(msg, Some(der), true).ok()?;
                let mut len = 0u32;
                CryptMsgGetParam(msg, CMSG_SIGNER_AUTH_ATTR_PARAM, 0, None, &mut len).ok()?;
                let mut buf = vec![0u64; (len as usize).div_ceil(8)];
                CryptMsgGetParam(
                    msg,
                    CMSG_SIGNER_AUTH_ATTR_PARAM,
                    0,
                    Some(buf.as_mut_ptr().cast()),
                    &mut len,
                )
                .ok()?;
                let attrs = &*(buf.as_ptr() as *const CRYPT_ATTRIBUTES);
                let want = std::ffi::CStr::from_ptr(szOID_RSA_signingTime.0.cast());
                for i in 0..attrs.cAttr as usize {
                    let a = &*attrs.rgAttr.add(i);
                    if std::ffi::CStr::from_ptr(a.pszObjId.0.cast()) != want || a.cValue == 0 {
                        continue;
                    }
                    let v = &*a.rgValue;
                    let enc = std::slice::from_raw_parts(v.pbData, v.cbData as usize);
                    let mut ft = FILETIME::default();
                    let mut size = std::mem::size_of::<FILETIME>() as u32;
                    CryptDecodeObjectEx(
                        X509_ASN_ENCODING,
                        PKCS_UTC_TIME,
                        enc,
                        0,
                        None,
                        Some((&mut ft as *mut FILETIME).cast()),
                        &mut size,
                    )
                    .ok()?;
                    return Some(ticks(ft));
                }
                None
            })();
            let _ = CryptMsgClose(Some(msg));
            found
        }
    }

    pub fn verify(data: &[u8], der: &[u8]) -> Result<Verdict, EngineError> {
        let when = signing_time(der);
        // SAFETY: inputs are borrowed slices that outlive the calls; the signer
        // certificate and the message's store are released below.
        unsafe {
            let para = CRYPT_VERIFY_MESSAGE_PARA {
                cbSize: std::mem::size_of::<CRYPT_VERIFY_MESSAGE_PARA>() as u32,
                dwMsgAndCertEncodingType: ENC,
                ..Default::default()
            };
            let ptrs = [data.as_ptr()];
            let lens = [data.len() as u32];
            let mut signer: *mut CERT_CONTEXT = std::ptr::null_mut();
            let ok = CryptVerifyDetachedMessageSignature(
                &para,
                0,
                der,
                1,
                ptrs.as_ptr(),
                lens.as_ptr(),
                Some(&mut signer),
            )
            .is_ok();
            let mut v = Verdict {
                signer: None,
                signed_at: when.map(|t| filetime_text(t, true)),
                intact: ok,
                trusted: false,
                trust_note: None,
            };
            if signer.is_null() {
                return Ok(v);
            }
            v.signer = Some(name(signer, false));
            // Trust: the chain from the signer, with the certificates the signature
            // carries, judged at the signing time when there is one.
            let extra = CryptGetMessageCertificates(ENC, None, 0, der).ok();
            let para = CERT_CHAIN_PARA {
                cbSize: std::mem::size_of::<CERT_CHAIN_PARA>() as u32,
                ..Default::default()
            };
            let at = when.map(filetime);
            let mut chain: *mut CERT_CHAIN_CONTEXT = std::ptr::null_mut();
            if CertGetCertificateChain(
                None,
                signer,
                at.as_ref().map(|t| t as *const FILETIME),
                extra,
                &para,
                0,
                None,
                &mut chain,
            )
            .is_ok()
                && !chain.is_null()
            {
                let e = (*chain).TrustStatus.dwErrorStatus
                    & !(CERT_TRUST_REVOCATION_STATUS_UNKNOWN | CERT_TRUST_IS_OFFLINE_REVOCATION);
                v.trusted = e == 0;
                v.trust_note = (e != 0).then(|| {
                    if e & CERT_TRUST_IS_REVOKED != 0 {
                        "its certificate was revoked"
                    } else if e & CERT_TRUST_IS_NOT_SIGNATURE_VALID != 0 {
                        "its certificate chain is broken"
                    } else if e & CERT_TRUST_IS_UNTRUSTED_ROOT != 0 {
                        "this PC doesn't trust the certificate's root, e.g. a self-signed one"
                    } else if e & CERT_TRUST_IS_PARTIAL_CHAIN != 0 {
                        "the certificate's issuer isn't known on this PC"
                    } else if e & CERT_TRUST_IS_NOT_TIME_VALID != 0 {
                        "the certificate wasn't valid when it signed"
                    } else {
                        "Windows couldn't confirm the certificate"
                    }
                    .to_string()
                });
                CertFreeCertificateChain(chain);
            }
            if let Some(s) = extra {
                let _ = CertCloseStore(Some(s), 0);
            }
            let _ = CertFreeCertificateContext(Some(signer));
            Ok(v)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::pdfedit::sign::{Setup, Visible, prepare, signatures};
        use windows::core::PCWSTR;

        /// A throwaway signer: an ephemeral CNG key (never written anywhere) and a
        /// self-signed certificate made in memory, not added to any store. Freeing the
        /// certificate frees the key; the provider is freed after it.
        struct TestSigner(*mut CERT_CONTEXT, NCRYPT_PROV_HANDLE);

        impl TestSigner {
            fn new() -> TestSigner {
                // SAFETY: test-only FFI; handles are freed in Drop or right here.
                unsafe {
                    let mut prov = NCRYPT_PROV_HANDLE::default();
                    NCryptOpenStorageProvider(&mut prov, MS_KEY_STORAGE_PROVIDER, 0).unwrap();
                    let mut key = NCRYPT_KEY_HANDLE::default();
                    NCryptCreatePersistedKey(
                        prov,
                        &mut key,
                        BCRYPT_RSA_ALGORITHM,
                        PCWSTR::null(),
                        CERT_KEY_SPEC(0),
                        NCRYPT_FLAGS(0),
                    )
                    .unwrap();
                    NCryptSetProperty(
                        NCRYPT_HANDLE(key.0),
                        NCRYPT_LENGTH_PROPERTY,
                        &2048u32.to_le_bytes(),
                        NCRYPT_FLAGS(0),
                    )
                    .unwrap();
                    NCryptFinalizeKey(key, NCRYPT_FLAGS(0)).unwrap();
                    let mut len = 0u32;
                    let x500 = w!("CN=Test Signer, O=Example");
                    CertStrToNameW(
                        X509_ASN_ENCODING,
                        x500,
                        CERT_X500_NAME_STR,
                        None,
                        None,
                        &mut len,
                        None,
                    )
                    .unwrap();
                    let mut name = vec![0u8; len as usize];
                    CertStrToNameW(
                        X509_ASN_ENCODING,
                        x500,
                        CERT_X500_NAME_STR,
                        None,
                        Some(name.as_mut_ptr()),
                        &mut len,
                        None,
                    )
                    .unwrap();
                    let blob = CRYPT_INTEGER_BLOB {
                        cbData: len,
                        pbData: name.as_mut_ptr(),
                    };
                    let alg = CRYPT_ALGORITHM_IDENTIFIER {
                        pszObjId: PSTR(szOID_RSA_SHA256RSA.0 as *mut u8),
                        Parameters: CRYPT_INTEGER_BLOB::default(),
                    };
                    let cert = CertCreateSelfSignCertificate(
                        Some(HCRYPTPROV_OR_NCRYPT_KEY_HANDLE(key.0)),
                        &blob,
                        CERT_CREATE_SELFSIGN_NO_KEY_INFO,
                        None,
                        Some(&alg),
                        None,
                        None,
                        None,
                    );
                    assert!(!cert.is_null(), "self-signed certificate");
                    // Hand the key to the certificate as its key context: it's freed with
                    // the certificate.
                    let kc = CERT_KEY_CONTEXT {
                        cbSize: std::mem::size_of::<CERT_KEY_CONTEXT>() as u32,
                        Anonymous: CERT_KEY_CONTEXT_0 { hNCryptKey: key },
                        dwKeySpec: CERT_NCRYPT_KEY_SPEC.0,
                    };
                    CertSetCertificateContextProperty(
                        cert,
                        CERT_KEY_CONTEXT_PROP_ID,
                        0,
                        Some((&kc as *const CERT_KEY_CONTEXT).cast()),
                    )
                    .unwrap();
                    TestSigner(cert, prov)
                }
            }
        }

        impl Drop for TestSigner {
            fn drop(&mut self) {
                // SAFETY: the certificate (and with it the key) is freed once.
                unsafe {
                    let _ = CertFreeCertificateContext(Some(self.0));
                    // The key keeps using its provider, so the provider goes last.
                    let _ = NCryptFreeObject(NCRYPT_HANDLE(self.1.0));
                }
            }
        }

        #[test]
        fn signs_verifies_and_catches_tampering() {
            let s = TestSigner::new();
            let data = b"the signed bytes".to_vec();
            // SAFETY: the test signer's context is valid while `s` lives.
            let der = unsafe { sign_with(s.0, &data) }.unwrap();
            let v = verify(&data, &der).unwrap();
            assert!(v.intact, "{v:?}");
            assert_eq!(v.signer.as_deref(), Some("Test Signer"));
            assert!(!v.trusted, "a self-signed certificate isn't trusted");
            assert!(v.trust_note.is_some());
            assert!(v.signed_at.as_deref().is_some_and(|t| t.ends_with("UTC")));
            let mut bad = data.clone();
            bad[0] ^= 1;
            assert!(!verify(&bad, &der).unwrap().intact);
        }

        #[test]
        fn a_signed_pdf_checks_out_and_shows_changes() {
            let s = TestSigner::new();
            let d = tempfile::tempdir().unwrap();
            let p = d.path().join("a.pdf");
            crate::pdfedit::testing::sample(&p, 2, false);
            let setup = Setup {
                name: "Test Signer".into(),
                reason: Some("Approved".into()),
                location: None,
                time: crate::pdfedit::sign::now(),
                visible: Some(Visible {
                    page: 0,
                    rect: [320.0, 40.0, 560.0, 100.0],
                }),
            };
            let prepared = prepare(&std::fs::read(&p).unwrap(), &setup).unwrap();
            // SAFETY: as above.
            let der = unsafe { sign_with(s.0, &prepared.signed_data()) }.unwrap();
            let signed = prepared.finish(&der).unwrap();
            let found = signatures(&signed).unwrap();
            assert_eq!(found.len(), 1);
            let v = verify(&found[0].data, &found[0].der).unwrap();
            assert!(v.intact && found[0].whole_file, "{v:?}");
            // Change one byte inside the signed part (in the original content).
            let mut tampered = signed.clone();
            let at = tampered
                .windows(4)
                .position(|w| w == b"/Pag")
                .expect("a page dictionary");
            tampered[at + 1] = b'Q';
            if let Ok(f) = signatures(&tampered)
                && let Some(f) = f.first()
            {
                let v = verify(&f.data, &f.der).unwrap();
                assert!(!v.intact, "tampering shows");
            }
            // Nothing was added to the user's store.
            let mine = super::certificates().unwrap_or_default();
            assert!(!mine.iter().any(|c| c.subject == "Test Signer"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cert(s: &str, current: bool, exp: &str) -> Cert {
        Cert {
            subject: s.into(),
            issuer: "CA".into(),
            expires: exp.into(),
            thumbprint: format!("AB{:02X}CD", s.len()),
            current,
        }
    }

    #[test]
    fn chooses_a_certificate() {
        let certs = vec![
            cert("Ada Example", true, "2027-01-01"),
            cert("Ada Example (old)", false, "2024-01-01"),
            cert("Work Signing", true, "2028-01-01"),
        ];
        assert!(choose(&certs, None).is_err(), "two current ones: ask");
        assert_eq!(
            choose(&certs, Some("work")).unwrap().subject,
            "Work Signing"
        );
        assert_eq!(choose(&certs, Some("ada")).unwrap().subject, "Ada Example");
        assert!(
            choose(&certs, Some("nobody"))
                .unwrap_err()
                .to_string()
                .contains("Work Signing")
        );
        assert_eq!(
            choose(&certs[..2], None).unwrap().subject,
            "Ada Example",
            "the only current one"
        );
        assert!(choose(&[], None).is_err());
        assert!(
            choose(&[cert("Old", false, "2020-01-01")], None)
                .unwrap_err()
                .to_string()
                .contains("expired")
        );
    }

    #[test]
    fn filetimes() {
        let t = unix_to_filetime(1_794_300_000, 0);
        assert_eq!(filetime_text(t, false), "2026-11-10");
        assert!(filetime_text(t, true).ends_with("UTC"));
    }

    #[test]
    fn reports_read_like_sentences() {
        let f = Found {
            field: "Signature1".into(),
            name: Some("Ada".into()),
            time: Some("D:20261110093000Z".into()),
            reason: Some("Approved".into()),
            location: None,
            sub_filter: "adbe.pkcs7.detached".into(),
            range_ok: true,
            whole_file: false,
            data: Vec::new(),
            der: vec![1],
        };
        let v = Verdict {
            signer: Some("Ada Example".into()),
            signed_at: Some("2026-11-10 09:30 UTC".into()),
            intact: true,
            trusted: false,
            trust_note: Some("self-signed".into()),
        };
        let r = report(&[(f.clone(), Some(v))]);
        assert!(
            r[0].contains("Ada Example on 2026-11-10 09:30 UTC"),
            "{r:?}"
        );
        assert!(r[0].contains("changed after this signature"));
        assert!(r[0].contains("not trusted (self-signed)"));
        let r = report(&[(f, None)]);
        assert!(r[0].contains("Ada on 2026-11-10 09:30 UTC"), "{r:?}");
    }
}
