//! Track tags: the artist, title, album, BPM and key that DJ software shows.
//!
//! The data types and the key parser are always built. Reading the tags out
//! of a file needs the `tags` feature, which pulls in `lofty`; the privileged
//! helper leaves it off because it never reads music.

use serde::{Deserialize, Serialize};

/// What a file's tags say about the track. Every field is optional because
/// plenty of DJ files carry no tags at all.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TrackTags {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<u16>,
    /// Beats per minute, kept only inside a range a DJ track can have.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bpm: Option<f32>,
    /// The key in standard notation, such as "Am" or "F#", whatever notation
    /// the file used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

impl TrackTags {
    pub fn is_empty(&self) -> bool {
        *self == TrackTags::default()
    }
}

/// Lowest and highest tempo accepted from a tag. Taggers write 0 for
/// "unknown", and a few write the BPM times 100.
const BPM_RANGE: std::ops::RangeInclusive<f32> = 40.0..=300.0;

/// A tempo from a tag's text, rounded to hundredths.
pub fn parse_bpm(text: &str) -> Option<f32> {
    let value: f32 = text.trim().replace(',', ".").parse().ok()?;
    let value =
        if value > *BPM_RANGE.end() && (value / 100.0) >= *BPM_RANGE.start() && (value / 100.0) <= *BPM_RANGE.end() {
            value / 100.0
        } else {
            value
        };
    BPM_RANGE.contains(&value).then(|| (value * 100.0).round() / 100.0)
}

/// Names of the twelve major and minor keys in the order of their tonic
/// (C = 0), spelled as rekordbox and Mixed In Key spell them.
const MAJOR: [&str; 12] = ["C", "Db", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"];
const MINOR: [&str; 12] = ["Cm", "C#m", "Dm", "Ebm", "Em", "Fm", "F#m", "Gm", "G#m", "Am", "Bbm", "Bm"];

/// A musical key: its tonic as a pitch class (C = 0) and whether it is minor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MusicalKey {
    pub tonic: u8,
    pub minor: bool,
}

impl MusicalKey {
    /// Standard notation, such as "Am" or "F#".
    pub fn name(self) -> &'static str {
        if self.minor {
            MINOR[usize::from(self.tonic % 12)]
        } else {
            MAJOR[usize::from(self.tonic % 12)]
        }
    }

    /// The Camelot wheel position, such as "8A" for A minor.
    pub fn camelot(self) -> String {
        // Each step round the wheel is a fifth (7 semitones); C major is 8B
        // and A minor, its relative, is 8A.
        let relative_major = if self.minor { (self.tonic + 3) % 12 } else { self.tonic % 12 };
        let steps = (u32::from(relative_major) * 7) % 12;
        format!("{}{}", (steps + 7) % 12 + 1, if self.minor { 'A' } else { 'B' })
    }

    /// Parse the key text a tagger wrote: standard notation ("Am", "A minor",
    /// "Bbm", "F#"), Camelot ("8A") or Open Key ("1m"). Combined forms such
    /// as "8A - Am" are read from their first part that parses.
    pub fn parse(text: &str) -> Option<MusicalKey> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        if let Some(key) = Self::parse_one(text) {
            return Some(key);
        }
        text.split(['/', ',', '-', '|', '(', ')'])
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .find_map(Self::parse_one)
    }

    fn parse_one(text: &str) -> Option<MusicalKey> {
        let lower = text.to_ascii_lowercase().replace(' ', "");
        // Camelot and Open Key: a number from 1 to 12, then a letter.
        let digits: String = lower.chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty() {
            let n: u32 = digits.parse().ok()?;
            let rest = &lower[digits.len()..];
            if !(1..=12).contains(&n) {
                return None;
            }
            let (camelot, minor) = match rest {
                "a" => (n, true),
                "b" => (n, false),
                // Open Key 1d is C major, which Camelot calls 8B.
                "m" => ((n + 6) % 12 + 1, true),
                "d" => ((n + 6) % 12 + 1, false),
                _ => return None,
            };
            let major_tonic = ((camelot + 4) * 7) % 12;
            let tonic = if minor { (major_tonic + 9) % 12 } else { major_tonic };
            return Some(MusicalKey { tonic: tonic as u8, minor });
        }
        let mut chars = lower.chars();
        let base = match chars.next()? {
            'c' => 0,
            'd' => 2,
            'e' => 4,
            'f' => 5,
            'g' => 7,
            'a' => 9,
            'b' => 11,
            _ => return None,
        };
        let rest: String = chars.collect();
        let (shift, mode) = if let Some(m) = rest.strip_prefix('#').or_else(|| rest.strip_prefix('♯')) {
            (1i8, m)
        } else if let Some(m) = rest.strip_prefix("♭") {
            (-1, m)
        } else if let Some(m) = rest.strip_prefix('b') {
            (-1, m)
        } else {
            (0, rest.as_str())
        };
        let minor = match mode {
            "" | "maj" | "major" => false,
            "m" | "min" | "minor" => true,
            _ => return None,
        };
        let tonic = (base + shift + 12) % 12;
        Some(MusicalKey { tonic: tonic as u8, minor })
    }
}

/// Read the tags of one file. Returns `None` when the file has no tag this
/// reader understands or can't be parsed; a scan never fails because of tags.
#[cfg(feature = "tags")]
pub fn read_tags(path: &std::path::Path) -> Option<TrackTags> {
    use lofty::config::{ParseOptions, ParsingMode};
    use lofty::prelude::{Accessor, ItemKey, TaggedFileExt};

    // Only the tag: no audio properties, which the header probe already
    // has, and no cover art, which can be megabytes.
    let options = ParseOptions::new().read_properties(false).read_cover_art(false).parsing_mode(ParsingMode::Relaxed);
    let file = lofty::probe::Probe::open(path).ok()?.options(options).read().ok()?;
    let tag = file.primary_tag().or_else(|| file.first_tag())?;
    let text = |value: Option<std::borrow::Cow<'_, str>>| {
        value
            .map(|v| v.trim().replace('\0', ""))
            .filter(|v| !v.is_empty())
            .map(|v| v.chars().take(300).collect::<String>())
    };
    let year = [ItemKey::Year, ItemKey::RecordingDate, ItemKey::OriginalReleaseDate]
        .into_iter()
        .filter_map(|k| tag.get_string(k))
        .find_map(|v| v.trim().get(..4).and_then(|y| y.parse::<u16>().ok()))
        .filter(|y| (1900..=2100).contains(y));
    let bpm = [ItemKey::Bpm, ItemKey::IntegerBpm].into_iter().filter_map(|k| tag.get_string(k)).find_map(parse_bpm);
    let key = tag.get_string(ItemKey::InitialKey).and_then(MusicalKey::parse).map(|k| k.name().to_string());
    let tags = TrackTags {
        title: text(tag.title()),
        artist: text(tag.artist()),
        album: text(tag.album()),
        genre: text(tag.genre()),
        year,
        bpm,
        key,
    };
    (!tags.is_empty()).then_some(tags)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(text: &str) -> Option<(&'static str, String)> {
        MusicalKey::parse(text).map(|k| (k.name(), k.camelot()))
    }

    #[test]
    fn keys_in_every_notation_agree() {
        for text in ["Am", "A minor", "Amin", "8A", "8a", "1m", "8A - Am", "Am/8A"] {
            assert_eq!(key(text), Some(("Am", "8A".to_string())), "{text}");
        }
        for text in ["C", "C major", "Cmaj", "8B", "1d"] {
            assert_eq!(key(text), Some(("C", "8B".to_string())), "{text}");
        }
        assert_eq!(key("F#m"), Some(("F#m", "11A".to_string())));
        assert_eq!(key("Gbm"), Some(("F#m", "11A".to_string())));
        assert_eq!(key("Bbm"), Some(("Bbm", "3A".to_string())));
        assert_eq!(key("A#m"), Some(("Bbm", "3A".to_string())));
        assert_eq!(key("Db"), Some(("Db", "3B".to_string())));
        assert_eq!(key("C#"), Some(("Db", "3B".to_string())));
        assert_eq!(key("B"), Some(("B", "1B".to_string())));
        assert_eq!(key("Abm"), Some(("G#m", "1A".to_string())));
        assert_eq!(key("E"), Some(("E", "12B".to_string())));
        assert_eq!(key("12A"), Some(("C#m", "12A".to_string())));
        assert_eq!(key("2m"), Some(("Em", "9A".to_string())));
        assert_eq!(key("6m"), Some(("G#m", "1A".to_string())));
    }

    #[test]
    fn every_camelot_position_round_trips() {
        for n in 1..=12 {
            for letter in ['A', 'B'] {
                let text = format!("{n}{letter}");
                let parsed = MusicalKey::parse(&text).map(|k| k.camelot());
                assert_eq!(parsed.as_deref(), Some(text.as_str()));
            }
        }
    }

    #[test]
    fn nonsense_keys_are_left_out() {
        for text in ["", "o", "off", "13A", "0B", "H", "Am7", "unknown"] {
            assert_eq!(MusicalKey::parse(text), None, "{text}");
        }
    }

    #[test]
    fn tempos_outside_a_dj_range_are_left_out() {
        assert_eq!(parse_bpm("128"), Some(128.0));
        assert_eq!(parse_bpm(" 123.456 "), Some(123.46));
        assert_eq!(parse_bpm("174,00"), Some(174.0));
        assert_eq!(parse_bpm("12800"), Some(128.0));
        assert_eq!(parse_bpm("0"), None);
        assert_eq!(parse_bpm("fast"), None);
        assert_eq!(parse_bpm("999999"), None);
    }
}
