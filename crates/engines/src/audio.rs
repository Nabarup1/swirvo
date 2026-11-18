//! Audio → WAV / FLAC in pure Rust: symphonia decodes (MP3, AAC/M4A, FLAC, Vorbis, WAV, AIFF,
//! CAF, ALAC), hound writes WAV, flacenc writes FLAC. Lossy targets (MP3, AAC, Opus) need the
//! Media pack (FFmpeg).

use std::fs::File;

use swirvo_core::{Capability, Engine, EngineError, StepRequest};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_NULL, DecoderOptions};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

const SOURCES: &[&str] = &["mp3", "m4a", "aac", "flac", "ogg", "wav", "aiff", "caf"];
const LOSSLESS_SOURCES: &[&str] = &["flac", "wav", "aiff", "caf"];
/// Samples kept in memory for FLAC encoding (~45 min of 48 kHz stereo).
const MAX_FLAC_SAMPLES: usize = 260_000_000;

pub struct AudioEngine;

impl Engine for AudioEngine {
    fn id(&self) -> &'static str {
        "audio"
    }

    fn capabilities(&self) -> Vec<Capability> {
        let mut caps = Vec::new();
        for &from in SOURCES {
            for to in ["wav", "flac"] {
                if from != to {
                    let note = if LOSSLESS_SOURCES.contains(&from) {
                        "lossless"
                    } else {
                        "decoded audio, kept exactly (file gets bigger)"
                    };
                    caps.push(Capability::new("audio", from, to).time(30, 300).note(note));
                }
            }
        }
        caps
    }

    fn run(&self, req: &StepRequest<'_>) -> Result<(), EngineError> {
        let mss = MediaSourceStream::new(Box::new(File::open(req.input)?), Default::default());
        let mut hint = Hint::new();
        hint.with_extension(req.from);
        let probed = symphonia::default::get_probe()
            .format(
                &hint,
                mss,
                &FormatOptions::default(),
                &MetadataOptions::default(),
            )
            .map_err(EngineError::decode)?;
        let mut format = probed.format;
        let track = format
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            .ok_or_else(|| EngineError::Decode("no audio track".into()))?;
        let track_id = track.id;
        let params = track.codec_params.clone();
        let mut decoder = symphonia::default::get_codecs()
            .make(&params, &DecoderOptions::default())
            .map_err(EngineError::decode)?;
        let rate = params
            .sample_rate
            .ok_or_else(|| EngineError::Decode("unknown sample rate".into()))?;
        // Keep 24-bit sources at 24 bits; everything else (incl. lossy) becomes 16-bit.
        let bits: u32 = if LOSSLESS_SOURCES.contains(&req.from)
            && params.bits_per_sample.is_some_and(|b| b > 16)
        {
            24
        } else {
            16
        };
        let shift = 32 - bits;

        let mut channels = params.channels.map(|c| c.count()).unwrap_or(0);
        let mut wav: Option<hound::WavWriter<std::io::BufWriter<File>>> = None;
        let mut pcm: Vec<i32> = Vec::new();
        let mut buf: Option<SampleBuffer<i32>> = None;
        loop {
            req.cancel.check()?;
            let packet = match format.next_packet() {
                Ok(p) => p,
                Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(SymError::ResetRequired) => break,
                Err(e) => return Err(EngineError::decode(e)),
            };
            if packet.track_id() != track_id {
                continue;
            }
            let decoded = match decoder.decode(&packet) {
                Ok(d) => d,
                Err(SymError::DecodeError(_)) => continue, // skip a damaged frame, like players do
                Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(EngineError::decode(e)),
            };
            let spec = *decoded.spec();
            channels = spec.channels.count();
            let sb = buf
                .get_or_insert_with(|| SampleBuffer::<i32>::new(decoded.capacity() as u64, spec));
            if sb.capacity() < decoded.capacity() * channels {
                *sb = SampleBuffer::<i32>::new(decoded.capacity() as u64, spec);
            }
            sb.copy_interleaved_ref(decoded);
            match req.to {
                "wav" => {
                    let w = match wav.as_mut() {
                        Some(w) => w,
                        None => {
                            let spec = hound::WavSpec {
                                channels: channels as u16,
                                sample_rate: rate,
                                bits_per_sample: bits as u16,
                                sample_format: hound::SampleFormat::Int,
                            };
                            wav.insert(
                                hound::WavWriter::create(req.output, spec)
                                    .map_err(EngineError::encode)?,
                            )
                        }
                    };
                    for &s in sb.samples() {
                        w.write_sample(s >> shift).map_err(EngineError::encode)?;
                    }
                }
                _ => {
                    pcm.extend(sb.samples().iter().map(|&s| s >> shift));
                    if pcm.len() > MAX_FLAC_SAMPLES {
                        return Err(EngineError::Invalid(
                            "too long for the built-in FLAC encoder; the Media pack handles it"
                                .into(),
                        ));
                    }
                }
            }
        }
        if channels == 0 {
            return Err(EngineError::Decode("no audio decoded".into()));
        }
        match req.to {
            "wav" => match wav {
                Some(w) => w.finalize().map_err(EngineError::encode)?,
                None => return Err(EngineError::Decode("no audio decoded".into())),
            },
            "flac" => write_flac(&pcm, channels, bits as usize, rate as usize, req.output)?,
            other => {
                return Err(EngineError::Invalid(format!(
                    "audio engine can't write {other}"
                )));
            }
        }
        Ok(())
    }
}

fn write_flac(
    pcm: &[i32],
    channels: usize,
    bits: usize,
    rate: usize,
    out: &std::path::Path,
) -> Result<(), EngineError> {
    use flacenc::component::BitRepr;
    use flacenc::error::Verify;
    let config = flacenc::config::Encoder::default()
        .into_verified()
        .map_err(|e| EngineError::Encode(format!("{e:?}")))?;
    let source = flacenc::source::MemSource::from_samples(pcm, channels, bits, rate);
    let stream = flacenc::encode_with_fixed_block_size(&config, source, config.block_size)
        .map_err(|e| EngineError::Encode(format!("{e:?}")))?;
    let mut sink = flacenc::bitsink::ByteSink::new();
    stream
        .write(&mut sink)
        .map_err(|e| EngineError::Encode(format!("{e:?}")))?;
    let mut bytes = sink.as_slice().to_vec();
    // STREAMINFO's minimum block size must exclude the (shorter) final block (RFC 9639
    // §8.2). flacenc counts it, and strict readers such as symphonia then reject the file.
    // With fixed-size blocking the minimum equals the maximum.
    if bytes.len() > 12 && bytes.starts_with(b"fLaC") && bytes[4] & 0x7f == 0 {
        let max = [bytes[10], bytes[11]];
        bytes[8..10].copy_from_slice(&max);
    }
    std::fs::write(out, bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use swirvo_core::{CancelToken, Preset};

    #[test]
    fn wav_to_flac_to_wav_is_bit_exact() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("tone.wav");
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&src, spec).unwrap();
        let samples: Vec<i16> = (0..44_100)
            .flat_map(|i| {
                let v =
                    ((i as f32 * 440.0 * std::f32::consts::TAU / 44_100.0).sin() * 12_000.0) as i16;
                [v, -v]
            })
            .collect();
        for s in &samples {
            w.write_sample(*s).unwrap();
        }
        w.finalize().unwrap();

        let cancel = CancelToken::new();
        let run = |i: &std::path::Path, o: &std::path::Path, from: &str, to: &str| {
            AudioEngine
                .run(&StepRequest {
                    input: i,
                    output: o,
                    from,
                    to,
                    preset: Preset::Balanced,
                    pdf: Default::default(),
                    cancel: &cancel,
                })
                .unwrap()
        };
        let flac = d.path().join("tone.flac");
        let back = d.path().join("back.wav");
        run(&src, &flac, "wav", "flac");
        assert!(std::fs::metadata(&flac).unwrap().len() < std::fs::metadata(&src).unwrap().len());
        run(&flac, &back, "flac", "wav");
        let got: Vec<i16> = hound::WavReader::open(&back)
            .unwrap()
            .samples::<i16>()
            .map(Result::unwrap)
            .collect();
        assert_eq!(got, samples);
    }
}
