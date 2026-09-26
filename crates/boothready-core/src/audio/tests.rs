use super::fixtures::*;
use super::*;
use std::io::Cursor;
use std::process::Command;

fn probe_bytes(b: &[u8]) -> Result<AudioInfo, ProbeError> {
    probe_reader(Cursor::new(b), b.len() as u64)
}

#[test]
fn wav_pcm_and_extensible() {
    let info = probe_bytes(&wav(44_100, 16, 2, 44_100, false)).unwrap();
    assert_eq!(info.codec, Codec::Pcm);
    assert_eq!(info.sample_rate, Some(44_100));
    assert_eq!(info.bit_depth, Some(16));
    assert_eq!(info.channels, Some(2));
    assert!((info.duration_secs.unwrap() - 1.0).abs() < 1e-6);
    assert!(!info.wav_extensible);

    let info = probe_bytes(&wav(96_000, 24, 2, 9_600, true)).unwrap();
    assert_eq!(info.codec, Codec::Pcm);
    assert!(info.wav_extensible);
    assert_eq!(info.bit_depth, Some(24));
    assert_eq!(info.sample_rate, Some(96_000));
}

#[test]
fn wav_truncation_is_flagged() {
    let mut w = wav(48_000, 16, 2, 48_000, false);
    w.truncate(w.len() / 2);
    let info = probe_bytes(&w).unwrap();
    assert!(info.issues.iter().any(|i| i.contains("cut short")), "{:?}", info.issues);
}

#[test]
fn aiff_and_aifc_sowt() {
    let info = probe_bytes(&aiff(44_100, 24, 2, 88_200, false)).unwrap();
    assert_eq!(info.container, Container::Aiff);
    assert_eq!(info.codec, Codec::Pcm);
    assert_eq!(info.sample_rate, Some(44_100));
    assert_eq!(info.bit_depth, Some(24));
    assert!((info.duration_secs.unwrap() - 2.0).abs() < 1e-6);
    let info = probe_bytes(&aiff(48_000, 16, 2, 100, true)).unwrap();
    assert_eq!(info.sample_rate, Some(48_000));
    assert!(info.issues.iter().any(|i| i.contains("sowt")));
}

#[test]
fn extended_float_roundtrip() {
    for r in [8_000u32, 22_050, 44_100, 48_000, 88_200, 96_000, 176_400, 192_000] {
        assert_eq!(aiff::extended_to_f64(&aiff::f64_to_extended(r)), r as f64);
    }
}

#[test]
fn flac_streaminfo() {
    let info = probe_bytes(&flac(96_000, 24, 2, 96_000 * 3)).unwrap();
    assert_eq!(info.codec, Codec::Flac);
    assert_eq!(info.sample_rate, Some(96_000));
    assert_eq!(info.bit_depth, Some(24));
    assert_eq!(info.channels, Some(2));
    assert!((info.duration_secs.unwrap() - 3.0).abs() < 1e-6);
    assert!(info.issues.is_empty(), "{:?}", info.issues);
}

#[test]
fn mp3_cbr_with_id3() {
    let info = probe_bytes(&mp3_cbr(100)).unwrap();
    assert_eq!(info.codec, Codec::Mp3);
    assert_eq!(info.sample_rate, Some(44_100));
    assert_eq!(info.bitrate_kbps, Some(128));
    assert!(!info.vbr);
    let d = info.duration_secs.unwrap();
    assert!((d - 100.0 * 1152.0 / 44_100.0).abs() < 0.05, "{d}");
}

#[test]
fn m4a_variants() {
    let lc = probe_bytes(&m4a("aac-lc", 44_100, 10)).unwrap();
    assert_eq!(lc.codec, Codec::AacLc);
    assert_eq!(lc.sample_rate, Some(44_100));
    assert_eq!(lc.bitrate_kbps, Some(256));
    assert!((lc.duration_secs.unwrap() - 10.0).abs() < 1e-6);
    let he = probe_bytes(&m4a("he-aac", 48_000, 5)).unwrap();
    assert_eq!(he.codec, Codec::AacHe);
    let alac = probe_bytes(&m4a("alac", 96_000, 5)).unwrap();
    assert_eq!(alac.codec, Codec::Alac);
    assert_eq!(alac.bit_depth, Some(24));
    assert_eq!(alac.sample_rate, Some(96_000));
}

#[test]
fn empty_and_unknown_files() {
    assert!(matches!(probe_bytes(&[]), Err(ProbeError::Corrupt(_))));
    assert!(matches!(probe_bytes(b"hello world, not audio"), Err(ProbeError::Unrecognized)));
    let mut m = m4a("aac-lc", 44_100, 1);
    let moov_at = m.windows(4).position(|w| w == b"moov").unwrap() - 4;
    m.truncate(moov_at);
    assert!(matches!(probe_bytes(&m), Err(ProbeError::Corrupt(_))));
}

/// Every prefix and a few thousand single-byte mutations of every fixture
/// must return a result or an error, never panic or hang.
#[test]
fn parsers_survive_truncation_and_mutation() {
    let samples = vec![
        wav(44_100, 16, 2, 200, false),
        wav(96_000, 24, 2, 200, true),
        aiff(44_100, 16, 2, 200, false),
        aiff(48_000, 16, 2, 200, true),
        flac(44_100, 16, 2, 1000),
        mp3_cbr(4),
        m4a("aac-lc", 44_100, 1),
        m4a("alac", 48_000, 1),
    ];
    let mut x = 0x9E37_79B9u32;
    for s in &samples {
        let cut = s.len().min(600);
        for n in 0..cut {
            let _ = probe_bytes(&s[..n]);
        }
        for _ in 0..3000 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let mut m = s.clone();
            let i = x as usize % m.len().min(700);
            m[i] = (x >> 8) as u8;
            let _ = probe_bytes(&m);
        }
    }
}

fn ffmpeg_available() -> bool {
    Command::new("ffmpeg").arg("-version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// Cross-check against files produced by a real encoder.
#[test]
fn ffmpeg_generated_files() {
    if !ffmpeg_available() {
        eprintln!("skipping: ffmpeg not installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    // (file name, ffmpeg args, codec, rate, bit depth)
    let cases: &[(&str, &[&str], Codec, u32, Option<u16>)] = &[
        ("a.wav", &["-c:a", "pcm_s16le", "-ar", "44100"], Codec::Pcm, 44_100, Some(16)),
        ("b.wav", &["-c:a", "pcm_s24le", "-ar", "96000"], Codec::Pcm, 96_000, Some(24)),
        ("c.wav", &["-c:a", "pcm_f32le", "-ar", "48000"], Codec::PcmFloat, 48_000, Some(32)),
        ("d.aiff", &["-c:a", "pcm_s16be", "-ar", "44100"], Codec::Pcm, 44_100, Some(16)),
        ("e.aiff", &["-c:a", "pcm_s24be", "-ar", "48000"], Codec::Pcm, 48_000, Some(24)),
        ("f.flac", &["-c:a", "flac", "-ar", "44100", "-sample_fmt", "s16"], Codec::Flac, 44_100, Some(16)),
        ("g.flac", &["-c:a", "flac", "-ar", "96000", "-sample_fmt", "s32"], Codec::Flac, 96_000, Some(24)),
        ("h.mp3", &["-c:a", "libmp3lame", "-b:a", "320k", "-ar", "44100"], Codec::Mp3, 44_100, None),
        ("i.mp3", &["-c:a", "libmp3lame", "-q:a", "2", "-ar", "48000"], Codec::Mp3, 48_000, None),
        ("j.m4a", &["-c:a", "aac", "-b:a", "256k", "-ar", "44100"], Codec::AacLc, 44_100, None),
        ("k.m4a", &["-c:a", "alac", "-ar", "44100", "-sample_fmt", "s16p"], Codec::Alac, 44_100, Some(16)),
        ("l.ogg", &["-c:a", "libvorbis", "-ar", "44100"], Codec::Vorbis, 44_100, None),
        ("m.opus", &["-c:a", "libopus", "-ar", "48000"], Codec::Opus, 48_000, None),
    ];
    for (name, args, codec, rate, bits) in cases {
        let out = dir.path().join(name);
        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "sine=frequency=440:duration=3"]);
        if name.ends_with(".mp3") {
            cmd.args(["-id3v2_version", "3", "-metadata", "title=Test"]);
        }
        cmd.args(["-ac", "2"]).args(*args).arg(&out);
        let st = cmd.output().unwrap();
        if !st.status.success() {
            // Some ffmpeg builds lack an encoder; skip that case only.
            eprintln!("skipping {name}: {}", String::from_utf8_lossy(&st.stderr));
            continue;
        }
        let info = probe_file(&out).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(info.codec, *codec, "{name}");
        assert_eq!(info.sample_rate, Some(*rate), "{name}");
        if bits.is_some() {
            assert_eq!(info.bit_depth, *bits, "{name}");
        }
        assert_eq!(info.channels, Some(2), "{name}");
        let d = info.duration_secs.unwrap_or_else(|| panic!("{name}: no duration"));
        assert!((d - 3.0).abs() < 0.2, "{name}: duration {d}");
    }
}
