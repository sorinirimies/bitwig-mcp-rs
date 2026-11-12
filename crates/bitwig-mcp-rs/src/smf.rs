//! Standard MIDI files (SMF, format 1) from notes. Pure.

use crate::midi::Note;

/// Ticks per quarter note.
pub const PPQ: u16 = 480;

/// One track of the file.
#[derive(Debug, Clone, PartialEq)]
pub struct SmfTrack {
    /// Track name (meta event).
    pub name: String,
    /// MIDI channel 1-16.
    pub channel: u8,
    /// Notes, in beats.
    pub notes: Vec<Note>,
}

fn vlq(mut v: u32, out: &mut Vec<u8>) {
    let mut bytes = vec![(v & 0x7F) as u8];
    v >>= 7;
    while v > 0 {
        bytes.push(((v & 0x7F) as u8) | 0x80);
        v >>= 7;
    }
    bytes.reverse();
    out.extend(bytes);
}

fn chunk(kind: &[u8; 4], data: &[u8], out: &mut Vec<u8>) {
    out.extend_from_slice(kind);
    out.extend_from_slice(&u32::try_from(data.len()).unwrap_or(u32::MAX).to_be_bytes());
    out.extend_from_slice(data);
}

fn ticks(beats: f64) -> u32 {
    // negative or NaN beats clamp to 0; u32 covers ~2.4 million bars at 480 ppq
    (beats.max(0.0) * f64::from(PPQ))
        .round()
        .min(f64::from(u32::MAX)) as u32
}

fn meta(delta: u32, kind: u8, data: &[u8], out: &mut Vec<u8>) {
    vlq(delta, out);
    out.extend_from_slice(&[0xFF, kind]);
    vlq(u32::try_from(data.len()).unwrap_or(0), out);
    out.extend_from_slice(data);
}

/// A format-1 file: a tempo/time-signature track, then one track per [`SmfTrack`].
#[must_use]
pub fn write(tracks: &[SmfTrack], tempo_bpm: f64, numerator: u8, denominator: u8) -> Vec<u8> {
    let mut out = Vec::new();
    let mut header = Vec::new();
    header.extend_from_slice(&1u16.to_be_bytes());
    header.extend_from_slice(
        &u16::try_from(tracks.len() + 1)
            .unwrap_or(u16::MAX)
            .to_be_bytes(),
    );
    header.extend_from_slice(&PPQ.to_be_bytes());
    chunk(b"MThd", &header, &mut out);

    // conductor track
    let mut t0 = Vec::new();
    let us_per_beat = (60_000_000.0 / tempo_bpm.clamp(1.0, 1000.0)).round() as u32;
    meta(0, 0x51, &us_per_beat.to_be_bytes()[1..], &mut t0);
    let denom_pow = (denominator.max(1) as f64).log2().round() as u8;
    meta(0, 0x58, &[numerator.max(1), denom_pow, 24, 8], &mut t0);
    meta(0, 0x2F, &[], &mut t0);
    chunk(b"MTrk", &t0, &mut out);

    for track in tracks {
        let ch = track.channel.clamp(1, 16) - 1;
        // (tick, is_on, key, velocity): offs sort before ons at the same tick
        let mut events: Vec<(u32, bool, u8, u8)> = Vec::new();
        for n in &track.notes {
            let on = ticks(n.start_beats);
            let off = ticks(n.start_beats + n.length_beats).max(on + 1);
            events.push((on, true, n.note.min(127), n.velocity.clamp(1, 127)));
            events.push((off, false, n.note.min(127), 0));
        }
        events.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        let mut data = Vec::new();
        meta(0, 0x03, track.name.as_bytes(), &mut data);
        let mut last = 0;
        for (tick, is_on, key, vel) in events {
            vlq(tick - last, &mut data);
            last = tick;
            data.extend_from_slice(&[if is_on { 0x90 } else { 0x80 } | ch, key, vel]);
        }
        meta(0, 0x2F, &[], &mut data);
        chunk(b"MTrk", &data, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(note: u8, start: f64, len: f64) -> Note {
        Note {
            note,
            start_beats: start,
            length_beats: len,
            velocity: 100,
        }
    }

    #[test]
    fn vlq_examples_from_the_spec() {
        for (v, want) in [
            (0u32, vec![0x00]),
            (0x40, vec![0x40]),
            (0x7F, vec![0x7F]),
            (0x80, vec![0x81, 0x00]),
            (0x2000, vec![0xC0, 0x00]),
            (0x3FFF, vec![0xFF, 0x7F]),
            (0x0FFF_FFFF, vec![0xFF, 0xFF, 0xFF, 0x7F]),
        ] {
            let mut out = Vec::new();
            vlq(v, &mut out);
            assert_eq!(out, want, "{v:#x}");
        }
    }

    #[test]
    fn file_layout() {
        let f = write(
            &[SmfTrack {
                name: "Bass".into(),
                channel: 2,
                notes: vec![n(45, 0.0, 1.0), n(57, 1.0, 0.5)],
            }],
            120.0,
            4,
            4,
        );
        assert_eq!(&f[..4], b"MThd");
        assert_eq!(
            &f[8..14],
            &[0, 1, 0, 2, 0x01, 0xE0],
            "format 1, 2 tracks, 480 ppq"
        );
        let text = f.windows(4).filter(|w| w == b"MTrk").count();
        assert_eq!(text, 2);
        // tempo 120 = 500000 us per beat
        assert!(
            f.windows(6)
                .any(|w| w == [0xFF, 0x51, 0x03, 0x07, 0xA1, 0x20])
        );
        // channel 2 note on A1 at tick 0, its note off one beat later (VLQ 480 = 0x83 0x60)
        assert!(f.windows(4).any(|w| w == [0x00, 0x91, 45, 100]));
        assert!(f.windows(5).any(|w| w == [0x83, 0x60, 0x81, 45, 0]));
        assert!(f.windows(4).any(|w| w == b"Bass"));
    }

    #[test]
    fn repeated_notes_turn_off_before_on() {
        let f = write(
            &[SmfTrack {
                name: "x".into(),
                channel: 1,
                notes: vec![n(60, 0.0, 1.0), n(60, 1.0, 1.0)],
            }],
            120.0,
            4,
            4,
        );
        let off = f.windows(3).position(|w| w == [0x80, 60, 0]).expect("off");
        let second_on = f
            .windows(3)
            .rposition(|w| w == [0x90, 60, 100])
            .expect("on");
        assert!(off < second_on);
    }
}
