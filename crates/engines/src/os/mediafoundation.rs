//! Video and audio with Windows Media Foundation (docs/12 §8): the decoders and encoders
//! Windows ships (H.264, AAC, MP3, plus HEVC, VP9 or AV1 when their extensions are
//! installed), so we bundle no codec and need no codec licences of our own. Hardware
//! encoders are used when the GPU has them.
//!
//! One synchronous pipeline: a source reader decodes (and converts and scales), a sink
//! writer encodes and muxes. Animated GIFs take decoded RGB frames from the reader and
//! are encoded by us.

use std::path::Path;

use swirvo_core::{CancelToken, Capability, Engine, EngineError, Preset, StepRequest};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::Variant::{VT_I8, VT_UI8};

use crate::video::{Backend, Probe, SizePlan, Times};
use windows_core::{GUID, HSTRING, PCWSTR};

pub const VIDEO_IN: &[&str] = &["mp4", "mov", "mkv", "webm", "avi", "wmv", "3gp", "ts"];
pub const AUDIO_IN: &[&str] = &["mp3", "m4a", "aac", "wav", "wma", "flac", "ac3", "amr"];
const AUDIO_OUT: &[&str] = &["mp3", "m4a", "aac", "wav"];

/// The pipeline's output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Out {
    Mp4,
    Mp3,
    M4a,
    Aac,
    Wav,
    Gif,
}

fn out_kind(to: &str) -> Option<Out> {
    Some(match to {
        "mp4" => Out::Mp4,
        "mp3" => Out::Mp3,
        "m4a" => Out::M4a,
        "aac" => Out::Aac,
        "wav" => Out::Wav,
        "gif" => Out::Gif,
        _ => return None,
    })
}

pub struct MfEngine;

/// Media Foundation, started for the length of one job on the job's thread. COM is
/// initialised only if the thread had no apartment yet, and undone with the session,
/// so a caller's apartment is never changed.
struct Session {
    com: bool,
}

impl Session {
    fn start() -> Result<Session, EngineError> {
        // SAFETY: COM/MF start-up, balanced in Drop (or right here on failure).
        unsafe {
            // S_OK or S_FALSE both need a CoUninitialize; RPC_E_CHANGED_MODE (the
            // thread is already an STA) doesn't, and Media Foundation works there too.
            let com = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
            if let Err(e) = MFStartup(MF_VERSION, MFSTARTUP_FULL) {
                if com {
                    CoUninitialize();
                }
                return Err(EngineError::Invalid(format!(
                    "Media Foundation isn't available ({e})"
                )));
            }
            Ok(Session { com })
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: balances the successful MFStartup and CoInitializeEx above.
        unsafe {
            let _ = MFShutdown();
            if self.com {
                CoUninitialize();
            }
        }
    }
}

impl MfEngine {
    /// Present on Windows 10/11 (N editions without the Media Feature Pack lack it).
    /// Probed on a thread of its own: discovery runs on the app's UI thread, which must
    /// stay free to become the single-threaded apartment drag and drop needs.
    ///
    /// The app delay-loads the Media Foundation DLLs (so it starts on N editions), and a
    /// delay-loaded call into a missing DLL would crash: they are looked for first, and
    /// nothing calls into them unless all three are there.
    pub fn discover() -> Option<MfEngine> {
        if !system_dlls(&["mfplat.dll", "mfreadwrite.dll", "mf.dll"]) {
            return None;
        }
        std::thread::spawn(|| Session::start().is_ok())
            .join()
            .unwrap_or(false)
            .then_some(MfEngine)
    }
}

/// Are these DLLs in System32? (Loaded and freed again; the loader keeps no handle.)
fn system_dlls(names: &[&str]) -> bool {
    use windows::Win32::Foundation::FreeLibrary;
    use windows::Win32::System::LibraryLoader::{LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW};
    names.iter().all(|n| {
        let name = HSTRING::from(*n);
        // SAFETY: loads a system DLL by name from System32 only, frees it straight away.
        unsafe {
            match LoadLibraryExW(PCWSTR(name.as_ptr()), None, LOAD_LIBRARY_SEARCH_SYSTEM32) {
                Ok(h) => {
                    let _ = FreeLibrary(h);
                    true
                }
                Err(_) => false,
            }
        }
    })
}

fn mf(what: &str) -> impl Fn(windows_core::Error) -> EngineError + '_ {
    move |e| EngineError::Encode(format!("{what}: {}", e.message()))
}

fn pack(hi: u32, lo: u32) -> u64 {
    (u64::from(hi) << 32) | u64::from(lo)
}

fn unpack(v: u64) -> (u32, u32) {
    ((v >> 32) as u32, v as u32)
}

struct Streams {
    video: Option<u32>,
    audio: Option<u32>,
}

/// Find the first video and audio streams and select only the ones we want.
unsafe fn select(reader: &IMFSourceReader, want_video: bool, want_audio: bool) -> Streams {
    let mut s = Streams {
        video: None,
        audio: None,
    };
    for i in 0..64u32 {
        // SAFETY: querying stream i; an error means there are no more streams.
        let Ok(t) = (unsafe { reader.GetNativeMediaType(i, 0) }) else {
            break;
        };
        // SAFETY: reading an attribute of a live media type.
        let major = unsafe { t.GetGUID(&MF_MT_MAJOR_TYPE) }.unwrap_or_default();
        let take = if major == MFMediaType_Video && s.video.is_none() && want_video {
            s.video = Some(i);
            true
        } else if major == MFMediaType_Audio && s.audio.is_none() && want_audio {
            s.audio = Some(i);
            true
        } else {
            false
        };
        // SAFETY: (de)selecting a stream that exists.
        unsafe {
            let _ = reader.SetStreamSelection(i, take);
        }
    }
    s
}

/// Bitrate for H.264 at this size and frame rate (bits per pixel per frame by preset).
fn video_bitrate(w: u32, h: u32, fps: f64, preset: Preset) -> u32 {
    let bpp = match preset {
        Preset::Best => 0.14,
        Preset::Smallest => 0.05,
        _ => 0.09,
    };
    ((w as f64 * h as f64 * fps.clamp(1.0, 120.0) * bpp) as u32).clamp(300_000, 60_000_000)
}

fn audio_bytes_per_sec(preset: Preset) -> u32 {
    // The AAC and MP3 encoders take 12000/16000/20000/24000 bytes per second.
    match preset {
        Preset::Best => 24_000,
        Preset::Smallest => 12_000,
        _ => 20_000,
    }
}

/// The encoder's own output type closest to what we want: the encoders accept only the
/// exact types they list (the MP3 one also wants its WAVEFORMATEX details), so pick one
/// rather than describing it.
unsafe fn encoder_type(
    sub: &GUID,
    rate: u32,
    ch: u32,
    bytes: u32,
) -> Result<IMFMediaType, EngineError> {
    use windows_core::Interface;
    let flags =
        (MFT_ENUM_FLAG_SYNCMFT.0 | MFT_ENUM_FLAG_LOCALMFT.0 | MFT_ENUM_FLAG_SORTANDFILTER.0) as u32;
    // SAFETY: COM calls on objects we own; the collection holds media types.
    unsafe {
        let all = MFTranscodeGetAudioOutputAvailableTypes(sub, flags, None)
            .map_err(mf("audio encoder"))?;
        let n = all.GetElementCount().map_err(mf("audio encoder"))?;
        let mut best: Option<(u32, IMFMediaType)> = None;
        for i in 0..n {
            let Ok(t) = all.GetElement(i).and_then(|u| u.cast::<IMFMediaType>()) else {
                continue;
            };
            let r = t.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND).unwrap_or(0);
            let c = t.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).unwrap_or(0);
            let b = t.GetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND).unwrap_or(0);
            if r != rate || c != ch {
                continue;
            }
            let d = b.abs_diff(bytes);
            if best.as_ref().is_none_or(|(bd, _)| d < *bd) {
                best = Some((d, t));
            }
        }
        best.map(|(_, t)| t).ok_or_else(|| {
            EngineError::Encode(format!(
                "Windows' audio encoder doesn't take {rate} Hz {ch}-channel sound"
            ))
        })
    }
}

unsafe fn media_type(major: &GUID, sub: &GUID) -> Result<IMFMediaType, EngineError> {
    // SAFETY: creating and filling a fresh media type.
    unsafe {
        let t = MFCreateMediaType().map_err(mf("media type"))?;
        t.SetGUID(&MF_MT_MAJOR_TYPE, major)
            .map_err(mf("media type"))?;
        t.SetGUID(&MF_MT_SUBTYPE, sub).map_err(mf("media type"))?;
        Ok(t)
    }
}

/// A source reader that decodes, converts and scales (call after [`Session::start`]).
unsafe fn open_reader(input: &Path) -> Result<IMFSourceReader, EngineError> {
    let src = HSTRING::from(input.as_os_str());
    // SAFETY: Media Foundation calls with live COM objects; every result is checked.
    unsafe {
        let mut attrs = None;
        MFCreateAttributes(&mut attrs, 3).map_err(mf("attributes"))?;
        let attrs = attrs.ok_or_else(|| EngineError::Encode("attributes".into()))?;
        attrs
            .SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)
            .map_err(mf("attributes"))?;
        attrs
            .SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)
            .map_err(mf("attributes"))?;
        MFCreateSourceReaderFromURL(PCWSTR(src.as_ptr()), &attrs).map_err(|e| {
            EngineError::Decode(format!(
                "Windows can't read this file ({}); its codec may need an extension from the Microsoft Store",
                e.message()
            ))
        })
    }
}

/// `fixed` (the compress-to-size tool) sets the video bitrate, frame size and audio
/// bitrate instead of the preset.
fn transcode(
    input: &Path,
    output: &Path,
    out: Out,
    preset: Preset,
    fixed: Option<&SizePlan>,
    cancel: &CancelToken,
) -> Result<(), EngineError> {
    let _mf = Session::start()?;
    // SAFETY: Media Foundation calls with live COM objects; every result is checked.
    unsafe {
        let reader = open_reader(input)?;
        let want_video = matches!(out, Out::Mp4 | Out::Gif);
        let want_audio = !matches!(out, Out::Gif);
        let st = select(&reader, want_video, want_audio);
        if want_video && st.video.is_none() {
            return Err(EngineError::Decode("this file has no video".into()));
        }
        if !want_video && st.audio.is_none() {
            return Err(EngineError::Decode("this file has no sound".into()));
        }
        if out == Out::Gif {
            return gif(&reader, st.video.unwrap_or(0), output, cancel);
        }
        if out == Out::Wav {
            return wav(&reader, st.audio.unwrap_or(0), output, cancel);
        }

        // Decoded formats from the reader.
        let mut video_in = None;
        let mut video_out = None;
        if let Some(v) = st.video {
            let native = reader.GetNativeMediaType(v, 0).map_err(mf("video"))?;
            let (mut w, mut h) = unpack(
                native
                    .GetUINT64(&MF_MT_FRAME_SIZE)
                    .unwrap_or(pack(1280, 720)),
            );
            let (rn, rd) = unpack(native.GetUINT64(&MF_MT_FRAME_RATE).unwrap_or(pack(30, 1)));
            let fps = if rd == 0 { 30.0 } else { rn as f64 / rd as f64 };
            if let Some(f) = fixed {
                (w, h) = (f.width, f.height);
            }
            // The H.264 encoder takes up to 4096 × 2304, in even sizes.
            let k = (4096.0 / w as f64).min(2304.0 / h as f64).min(1.0);
            w = ((w as f64 * k) as u32 & !1).max(2);
            h = ((h as f64 * k) as u32 & !1).max(2);
            let t = media_type(&MFMediaType_Video, &MFVideoFormat_NV12)?;
            t.SetUINT64(&MF_MT_FRAME_SIZE, pack(w, h))
                .map_err(mf("video"))?;
            t.SetUINT64(&MF_MT_FRAME_RATE, pack(rn.max(1), rd.max(1)))
                .map_err(mf("video"))?;
            reader
                .SetCurrentMediaType(v, None, &t)
                .map_err(mf("video decoder"))?;
            let cur = reader.GetCurrentMediaType(v).map_err(mf("video"))?;
            let o = media_type(&MFMediaType_Video, &MFVideoFormat_H264)?;
            let rate = fixed.map_or_else(|| video_bitrate(w, h, fps, preset), |f| f.video_bps);
            o.SetUINT32(&MF_MT_AVG_BITRATE, rate).map_err(mf("video"))?;
            o.SetUINT64(&MF_MT_FRAME_SIZE, pack(w, h))
                .map_err(mf("video"))?;
            o.SetUINT64(&MF_MT_FRAME_RATE, pack(rn.max(1), rd.max(1)))
                .map_err(mf("video"))?;
            o.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))
                .map_err(mf("video"))?;
            o.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
                .map_err(mf("video"))?;
            video_in = Some(cur);
            video_out = Some(o);
        }
        let mut audio_in = None;
        let mut audio_out = None;
        if let Some(a) = st.audio {
            let native = reader.GetNativeMediaType(a, 0).map_err(mf("audio"))?;
            let ch = native
                .GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS)
                .unwrap_or(2)
                .clamp(1, 2);
            let rate = match native
                .GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND)
                .unwrap_or(48_000)
            {
                44_100 => 44_100,
                _ => 48_000,
            };
            let t = media_type(&MFMediaType_Audio, &MFAudioFormat_PCM)?;
            t.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)
                .map_err(mf("audio"))?;
            t.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, rate)
                .map_err(mf("audio"))?;
            t.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, ch)
                .map_err(mf("audio"))?;
            reader
                .SetCurrentMediaType(a, None, &t)
                .map_err(mf("audio decoder"))?;
            let cur = reader.GetCurrentMediaType(a).map_err(mf("audio"))?;
            let sub = if out == Out::Mp3 {
                MFAudioFormat_MP3
            } else {
                MFAudioFormat_AAC
            };
            let bytes = fixed.map_or_else(|| audio_bytes_per_sec(preset), |f| f.audio_bps / 8);
            let o = encoder_type(&sub, rate, ch, bytes)?;
            audio_in = Some(cur);
            audio_out = Some(o);
        }
        // The writer: container from the extension's kind.
        let mut wattrs = None;
        MFCreateAttributes(&mut wattrs, 3).map_err(mf("attributes"))?;
        let wattrs = wattrs.ok_or_else(|| EngineError::Encode("attributes".into()))?;
        let container = match out {
            Out::Mp3 => MFTranscodeContainerType_MP3,
            Out::Aac => MFTranscodeContainerType_ADTS,
            _ => MFTranscodeContainerType_MPEG4,
        };
        wattrs
            .SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &container)
            .map_err(mf("attributes"))?;
        wattrs
            .SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)
            .map_err(mf("attributes"))?;
        wattrs
            .SetUINT32(&MF_SINK_WRITER_DISABLE_THROTTLING, 1)
            .map_err(mf("attributes"))?;
        let dst = HSTRING::from(output.as_os_str());
        let writer = MFCreateSinkWriterFromURL(PCWSTR(dst.as_ptr()), None, &wattrs)
            .map_err(mf("can't create the output"))?;
        let mut map: Vec<(u32, u32)> = Vec::new();
        if let (Some(v), Some(i), Some(o)) = (st.video, &video_in, &video_out) {
            let idx = writer.AddStream(o).map_err(mf("H.264 encoder"))?;
            writer
                .SetInputMediaType(idx, i, None)
                .map_err(mf("H.264 encoder"))?;
            map.push((v, idx));
        }
        if let (Some(a), Some(i), Some(o)) = (st.audio, &audio_in, &audio_out) {
            let idx = writer.AddStream(o).map_err(mf(if out == Out::Mp3 {
                "MP3 encoder"
            } else {
                "AAC encoder"
            }))?;
            writer
                .SetInputMediaType(idx, i, None)
                .map_err(mf("audio encoder"))?;
            map.push((a, idx));
        }
        writer.BeginWriting().map_err(mf("writing"))?;
        let mut open = map.len();
        let mut first_ts: Option<i64> = None;
        while open > 0 {
            cancel.check()?;
            let (mut actual, mut flags, mut ts) = (0u32, 0u32, 0i64);
            let mut sample: Option<IMFSample> = None;
            reader
                .ReadSample(
                    MF_SOURCE_READER_ANY_STREAM.0 as u32,
                    0,
                    Some(&mut actual),
                    Some(&mut flags),
                    Some(&mut ts),
                    Some(&mut sample),
                )
                .map_err(mf("reading"))?;
            let Some(&(_, w)) = map.iter().find(|(r, _)| *r == actual) else {
                continue;
            };
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                open -= 1;
                continue;
            }
            // Start at zero: some sources begin at a non-zero time.
            let base = *first_ts.get_or_insert(ts);
            if flags & MF_SOURCE_READERF_STREAMTICK.0 as u32 != 0 {
                writer
                    .SendStreamTick(w, (ts - base).max(0))
                    .map_err(mf("writing"))?;
            }
            if let Some(s) = sample {
                let t = s.GetSampleTime().unwrap_or(ts);
                s.SetSampleTime((t - base).max(0)).map_err(mf("writing"))?;
                writer.WriteSample(w, &s).map_err(mf("encoding"))?;
            }
        }
        writer.Finalize().map_err(mf("finishing the file"))?;
    }
    Ok(())
}

/// Decoded PCM straight into a WAV file.
unsafe fn wav(
    reader: &IMFSourceReader,
    stream: u32,
    output: &Path,
    cancel: &CancelToken,
) -> Result<(), EngineError> {
    // SAFETY: as in `transcode`.
    unsafe {
        let t = media_type(&MFMediaType_Audio, &MFAudioFormat_PCM)?;
        t.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)
            .map_err(mf("audio"))?;
        reader
            .SetCurrentMediaType(stream, None, &t)
            .map_err(mf("audio decoder"))?;
        let cur = reader.GetCurrentMediaType(stream).map_err(mf("audio"))?;
        let spec = hound::WavSpec {
            channels: cur.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).unwrap_or(2) as u16,
            sample_rate: cur
                .GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND)
                .unwrap_or(48_000),
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(output, spec).map_err(EngineError::encode)?;
        loop {
            cancel.check()?;
            let mut flags = 0u32;
            let mut sample: Option<IMFSample> = None;
            reader
                .ReadSample(stream, 0, None, Some(&mut flags), None, Some(&mut sample))
                .map_err(mf("reading"))?;
            if let Some(s) = sample {
                let buf = s.ConvertToContiguousBuffer().map_err(mf("reading"))?;
                let mut p = std::ptr::null_mut();
                let mut len = 0u32;
                buf.Lock(&mut p, None, Some(&mut len))
                    .map_err(mf("reading"))?;
                let bytes = std::slice::from_raw_parts(p, len as usize);
                for c in bytes.as_chunks::<2>().0 {
                    w.write_sample(i16::from_le_bytes(*c))
                        .map_err(EngineError::encode)?;
                }
                let _ = buf.Unlock();
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                break;
            }
        }
        w.finalize().map_err(EngineError::encode)?;
    }
    Ok(())
}

/// An RGB32 sample (`w`×`h`, rows `stride` bytes apart, bottom-up when negative) as RGBA.
unsafe fn to_rgba(
    s: &IMFSample,
    w: u32,
    h: u32,
    stride: i32,
) -> Result<image::RgbaImage, EngineError> {
    // SAFETY: the buffer is locked while its bytes are read and unlocked after.
    unsafe {
        let buf = s.ConvertToContiguousBuffer().map_err(mf("reading"))?;
        let mut p = std::ptr::null_mut();
        let mut len = 0u32;
        buf.Lock(&mut p, None, Some(&mut len))
            .map_err(mf("reading"))?;
        let bytes = std::slice::from_raw_parts(p, len as usize);
        let row = (w * 4) as usize;
        let pitch = (stride.unsigned_abs() as usize).max(row);
        let mut rgba = image::RgbaImage::new(w, h);
        for y in 0..h as usize {
            let src_y = if stride < 0 { h as usize - 1 - y } else { y };
            let Some(line) = bytes.get(src_y * pitch..src_y * pitch + row) else {
                break;
            };
            for (x, px) in line.as_chunks::<4>().0.iter().enumerate() {
                rgba.put_pixel(x as u32, y as u32, image::Rgba([px[2], px[1], px[0], 255]));
            }
        }
        let _ = buf.Unlock();
        Ok(rgba)
    }
}

/// The reader's video stream set to RGB32 at `w`×`h` (even): (w, h, stride).
unsafe fn rgb_output(
    reader: &IMFSourceReader,
    stream: u32,
    w: u32,
    h: u32,
) -> Result<(u32, u32, i32), EngineError> {
    // SAFETY: as in `transcode`.
    unsafe {
        let t = media_type(&MFMediaType_Video, &MFVideoFormat_RGB32)?;
        t.SetUINT64(&MF_MT_FRAME_SIZE, pack(w, h))
            .map_err(mf("video"))?;
        reader
            .SetCurrentMediaType(stream, None, &t)
            .map_err(mf("video decoder"))?;
        let cur = reader.GetCurrentMediaType(stream).map_err(mf("video"))?;
        let (cw, ch) = unpack(cur.GetUINT64(&MF_MT_FRAME_SIZE).unwrap_or(pack(w, h)));
        // A negative (or absent) default stride means bottom-up rows.
        let stride = cur
            .GetUINT32(&MF_MT_DEFAULT_STRIDE)
            .map_or(-((cw * 4) as i32), |s| s as i32);
        Ok((cw, ch, stride))
    }
}

/// GIF limits: up to 480 px wide, 12 frames per second, 20 seconds.
const GIF_W: u32 = 480;
const GIF_FPS: f64 = 12.0;
const GIF_MAX_S: f64 = 20.0;

/// Decoded frames, scaled by the reader, encoded as an animated GIF.
unsafe fn gif(
    reader: &IMFSourceReader,
    stream: u32,
    output: &Path,
    cancel: &CancelToken,
) -> Result<(), EngineError> {
    // SAFETY: as in `transcode`.
    unsafe {
        let native = reader.GetNativeMediaType(stream, 0).map_err(mf("video"))?;
        let (sw, sh) = unpack(
            native
                .GetUINT64(&MF_MT_FRAME_SIZE)
                .unwrap_or(pack(640, 360)),
        );
        let k = (GIF_W as f64 / sw.max(1) as f64).min(1.0);
        let (w, h) = (
            ((sw as f64 * k) as u32 & !1).max(2),
            ((sh as f64 * k) as u32 & !1).max(2),
        );
        let t = media_type(&MFMediaType_Video, &MFVideoFormat_RGB32)?;
        t.SetUINT64(&MF_MT_FRAME_SIZE, pack(w, h))
            .map_err(mf("video"))?;
        reader
            .SetCurrentMediaType(stream, None, &t)
            .map_err(mf("video decoder"))?;
        let cur = reader.GetCurrentMediaType(stream).map_err(mf("video"))?;
        // A negative (or absent) default stride means bottom-up rows.
        let stride = cur
            .GetUINT32(&MF_MT_DEFAULT_STRIDE)
            .map_or(-((w * 4) as i32), |s| s as i32);
        let file = std::fs::File::create(output)?;
        let mut enc =
            image::codecs::gif::GifEncoder::new_with_speed(std::io::BufWriter::new(file), 20);
        enc.set_repeat(image::codecs::gif::Repeat::Infinite)
            .map_err(EngineError::encode)?;
        let step = (10_000_000.0 / GIF_FPS) as i64;
        let mut next = 0i64;
        let mut base: Option<i64> = None;
        loop {
            cancel.check()?;
            let mut flags = 0u32;
            let mut ts = 0i64;
            let mut sample: Option<IMFSample> = None;
            reader
                .ReadSample(
                    stream,
                    0,
                    None,
                    Some(&mut flags),
                    Some(&mut ts),
                    Some(&mut sample),
                )
                .map_err(mf("reading"))?;
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                break;
            }
            let Some(s) = sample else { continue };
            let t = ts - *base.get_or_insert(ts);
            if t as f64 / 10_000_000.0 > GIF_MAX_S {
                break;
            }
            if t < next {
                continue;
            }
            next += step;
            let rgba = to_rgba(&s, w, h, stride)?;
            let frame = image::Frame::from_parts(
                rgba,
                0,
                0,
                image::Delay::from_numer_denom_ms(1000, GIF_FPS as u32),
            );
            enc.encode_frame(frame).map_err(EngineError::encode)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- video tools

/// The video tools' backend (`crate::video`): probe, frames, encode to a size.
pub(crate) struct MfVideo;

/// Media Foundation time is in 100 ns units.
const UNITS: f64 = 10_000_000.0;

/// The presentation's duration in seconds.
unsafe fn duration(reader: &IMFSourceReader) -> Option<f64> {
    // SAFETY: reading a PROPVARIANT the reader filled in; its tag says which field is set.
    unsafe {
        let v = reader
            .GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION)
            .ok()?;
        let a = &v.Anonymous.Anonymous;
        (a.vt == VT_UI8).then(|| a.Anonymous.uhVal as f64 / UNITS)
    }
}

/// Move the reader to `t` seconds (it lands on the key frame at or before it).
unsafe fn seek(reader: &IMFSourceReader, t: f64) -> Result<(), EngineError> {
    // SAFETY: a VT_I8 PROPVARIANT owns nothing, so it needs no clearing.
    unsafe {
        let mut pv = PROPVARIANT::default();
        (*pv.Anonymous.Anonymous).vt = VT_I8;
        (*pv.Anonymous.Anonymous).Anonymous.hVal = (t.max(0.0) * UNITS) as i64;
        reader
            .SetCurrentPosition(&GUID::zeroed(), &pv)
            .map_err(mf("seeking"))
    }
}

fn even(v: f64) -> u32 {
    ((v / 2.0).round() as u32 * 2).max(2)
}

impl Backend for MfVideo {
    fn probe(&self, input: &Path, _cancel: &CancelToken) -> Result<Probe, EngineError> {
        let _mf = Session::start()?;
        // SAFETY: as in `transcode`.
        unsafe {
            let reader = open_reader(input)?;
            let st = select(&reader, true, true);
            let v = st
                .video
                .ok_or_else(|| EngineError::Decode("this file has no video".into()))?;
            let native = reader.GetNativeMediaType(v, 0).map_err(mf("video"))?;
            let (width, height) = unpack(native.GetUINT64(&MF_MT_FRAME_SIZE).map_err(mf("video"))?);
            let (rn, rd) = unpack(native.GetUINT64(&MF_MT_FRAME_RATE).unwrap_or(pack(30, 1)));
            let fps = if rn == 0 || rd == 0 {
                30.0
            } else {
                rn as f64 / rd as f64
            };
            let duration = duration(&reader).ok_or_else(|| {
                EngineError::Decode("couldn't tell how long this video is".into())
            })?;
            Ok(Probe {
                duration,
                width,
                height,
                fps,
                audio: st.audio.is_some(),
            })
        }
    }

    fn frames(
        &self,
        input: &Path,
        times: &Times,
        cancel: &CancelToken,
        each: &mut dyn FnMut(usize, image::RgbaImage) -> Result<(), EngineError>,
    ) -> Result<(), EngineError> {
        let _mf = Session::start()?;
        let list: Vec<f64> = match times {
            Times::At(l) => l.clone(),
            Times::Every { step, count } => (0..*count).map(|i| i as f64 * step).collect(),
        };
        // SAFETY: as in `transcode`.
        unsafe {
            let reader = open_reader(input)?;
            let v = select(&reader, true, false)
                .video
                .ok_or_else(|| EngineError::Decode("this file has no video".into()))?;
            let native = reader.GetNativeMediaType(v, 0).map_err(mf("video"))?;
            let (sw, sh) = unpack(
                native
                    .GetUINT64(&MF_MT_FRAME_SIZE)
                    .unwrap_or(pack(1280, 720)),
            );
            let (rn, rd) = unpack(native.GetUINT64(&MF_MT_FRAME_RATE).unwrap_or(pack(30, 1)));
            let frame_s = if rn == 0 {
                1.0 / 30.0
            } else {
                rd as f64 / rn as f64
            };
            // Full size, up to 4K wide.
            let k = (3840.0 / sw.max(1) as f64).min(1.0);
            let (w, h, stride) = rgb_output(&reader, v, even(sw as f64 * k), even(sh as f64 * k))?;
            // Time of the last decoded frame, to decode on instead of seeking when the next
            // wanted frame is just ahead.
            let mut pos: Option<f64> = None;
            for (i, &t) in list.iter().enumerate() {
                cancel.check()?;
                if !pos.is_some_and(|p| t >= p && t - p < 2.0) {
                    seek(&reader, t)?;
                }
                let mut last: Option<IMFSample> = None;
                let mut ended = false;
                loop {
                    cancel.check()?;
                    let (mut flags, mut ts) = (0u32, 0i64);
                    let mut sample: Option<IMFSample> = None;
                    reader
                        .ReadSample(
                            v,
                            0,
                            None,
                            Some(&mut flags),
                            Some(&mut ts),
                            Some(&mut sample),
                        )
                        .map_err(mf("reading"))?;
                    if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                        ended = true;
                        break;
                    }
                    let Some(s) = sample else { continue };
                    let at = s.GetSampleTime().unwrap_or(ts) as f64 / UNITS;
                    pos = Some(at);
                    last = Some(s);
                    if at + frame_s / 2.0 >= t {
                        break;
                    }
                }
                // Past the end: the last frame there is.
                match last {
                    Some(s) => each(i, to_rgba(&s, w, h, stride)?)?,
                    None => break,
                }
                if ended {
                    break;
                }
            }
        }
        Ok(())
    }

    fn encode(
        &self,
        input: &Path,
        output: &Path,
        plan: &SizePlan,
        cancel: &CancelToken,
    ) -> Result<(), EngineError> {
        transcode(
            input,
            output,
            Out::Mp4,
            Preset::Balanced,
            Some(plan),
            cancel,
        )
    }

    fn audio_rates(&self) -> &'static [u32] {
        // What Windows' AAC encoder takes (12000–24000 bytes per second).
        &[192_000, 160_000, 128_000, 96_000]
    }
}

impl Engine for MfEngine {
    fn id(&self) -> &'static str {
        "os-media"
    }

    fn capabilities(&self) -> Vec<Capability> {
        let mut caps = Vec::new();
        let note = "Windows' own encoders";
        for &from in VIDEO_IN {
            if from != "mp4" {
                caps.push(
                    Capability::new("os-media", from, "mp4")
                        .loss(10)
                        .time(200, 400)
                        .note(note),
                );
            }
            caps.push(
                Capability::new("os-media", from, "gif")
                    .loss(40)
                    .time(300, 600)
                    .note("animated GIF, 12 fps, up to 480 px wide, first 20 s"),
            );
            for &to in AUDIO_OUT {
                caps.push(
                    Capability::new("os-media", from, to)
                        .loss(if to == "wav" { 1 } else { 10 })
                        .time(100, 200)
                        .note("the soundtrack"),
                );
            }
        }
        for &from in AUDIO_IN {
            for &to in AUDIO_OUT {
                if from != to {
                    caps.push(
                        Capability::new("os-media", from, to)
                            .loss(if to == "wav" { 1 } else { 10 })
                            .time(60, 150)
                            .note(note),
                    );
                }
            }
        }
        caps
    }

    fn run(&self, req: &StepRequest<'_>) -> Result<(), EngineError> {
        let out = out_kind(req.to).ok_or_else(|| EngineError::Unsupported {
            engine: "os-media",
            from: req.from.into(),
            to: req.to.into(),
        })?;
        // Media Foundation picks the sink from the extension; outputs are temp files
        // with the right one already.
        transcode(req.input, req.output, out, req.preset, None, req.cancel)
    }
}
