use super::{le16, le32, le64, AudioInfo, Codec, Container, ProbeError, Source};
use std::io::{Read, Seek};

const WAVE_FORMAT_PCM: u16 = 0x0001;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 0x0003;
const WAVE_FORMAT_MPEGLAYER3: u16 = 0x0055;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

pub(super) fn probe<R: Read + Seek>(src: &mut Source<R>) -> Result<AudioInfo, ProbeError> {
    let head = src.read_exact_at(0, 12)?;
    let rf64 = &head[0..4] == b"RF64";
    let riff_size = le32(&head, 4) as u64;
    let mut info = AudioInfo::new(Container::Wav, Codec::Other);
    if rf64 {
        info.issues.push("RF64 (larger than 4 GB) WAV; most DJ hardware cannot read it".into());
    } else if riff_size + 8 > src.len {
        info.issues.push("file is shorter than its RIFF header says (truncated copy?)".into());
    }

    let mut pos = 12u64;
    let mut byte_rate = 0u32;
    let mut data_len: Option<u64> = None;
    let mut ds64_data_len: Option<u64> = None;
    let mut have_fmt = false;
    for _ in 0..512 {
        if pos + 8 > src.len {
            break;
        }
        let ch = src.read_exact_at(pos, 8)?;
        let id = [ch[0], ch[1], ch[2], ch[3]];
        let size = le32(&ch, 4) as u64;
        let body = pos + 8;
        match &id {
            b"ds64" if size >= 28 => {
                let d = src.read_exact_at(body, 28)?;
                ds64_data_len = Some(le64(&d, 8));
            }
            b"fmt " => {
                if size < 16 {
                    return Err(ProbeError::Corrupt("WAV format chunk is too short".into()));
                }
                let f = src.read_exact_at(body, size.min(40) as usize)?;
                let mut tag = le16(&f, 0);
                info.channels = Some(le16(&f, 2));
                info.sample_rate = Some(le32(&f, 4));
                byte_rate = le32(&f, 8);
                info.bit_depth = Some(le16(&f, 14));
                if tag == WAVE_FORMAT_EXTENSIBLE && f.len() >= 40 {
                    info.wav_extensible = true;
                    let valid_bits = le16(&f, 18);
                    if valid_bits != 0 {
                        info.bit_depth = Some(valid_bits);
                    }
                    // Sub-format GUID; its first two bytes are the real tag.
                    tag = le16(&f, 24);
                }
                info.codec = match tag {
                    WAVE_FORMAT_PCM => Codec::Pcm,
                    WAVE_FORMAT_IEEE_FLOAT => Codec::PcmFloat,
                    WAVE_FORMAT_MPEGLAYER3 => Codec::Mp3,
                    _ => Codec::Other,
                };
                have_fmt = true;
            }
            b"data" => {
                let declared = if rf64 && size == 0xFFFF_FFFF { ds64_data_len.unwrap_or(0) } else { size };
                let available = src.len.saturating_sub(body);
                if declared > available {
                    info.issues.push(format!("audio data is cut short ({} of {} bytes present)", available, declared));
                }
                data_len = Some(declared.min(available));
                if have_fmt {
                    break;
                }
            }
            _ => {}
        }
        if id == *b"data" && size == 0xFFFF_FFFF {
            break;
        }
        pos = body + size + (size & 1);
    }
    if !have_fmt {
        return Err(ProbeError::Corrupt("WAV file has no format chunk".into()));
    }
    let data_len = data_len.ok_or_else(|| ProbeError::Corrupt("WAV file has no audio data chunk".into()))?;
    if byte_rate > 0 {
        info.duration_secs = Some(data_len as f64 / byte_rate as f64);
        info.bitrate_kbps = Some((byte_rate as u64 * 8 / 1000) as u32);
    }
    if info.sample_rate == Some(0) || info.channels == Some(0) {
        return Err(ProbeError::Corrupt("WAV header declares zero channels or sample rate".into()));
    }
    Ok(info)
}
