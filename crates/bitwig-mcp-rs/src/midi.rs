//! Turning a note list into timed MIDI events. Pure.

/// One note to play or record.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Note {
    /// MIDI note number 0..=127.
    pub note: u8,
    /// Start in beats from the clip start.
    pub start_beats: f64,
    /// Length in beats.
    pub length_beats: f64,
    /// Velocity 1..=127.
    pub velocity: u8,
}

/// A note on/off at a time offset in seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Event {
    /// Seconds since the first event.
    pub at_secs: f64,
    /// MIDI note number.
    pub note: u8,
    /// Velocity; 0 means note off.
    pub velocity: u8,
}

/// Why a note list was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidNotes(pub String);

/// Checks the notes and orders them into on/off events at `tempo_bpm`.
/// Offs sort before ons at the same instant so repeated notes retrigger cleanly.
///
/// # Errors
/// [`InvalidNotes`] for a bad tempo, note, velocity, start or length.
pub fn plan(notes: &[Note], tempo_bpm: f64) -> Result<Vec<Event>, InvalidNotes> {
    if !(tempo_bpm.is_finite() && tempo_bpm > 0.0) {
        return Err(InvalidNotes(format!(
            "tempo must be positive, got {tempo_bpm}"
        )));
    }
    let secs_per_beat = 60.0 / tempo_bpm;
    let mut events = Vec::with_capacity(notes.len() * 2);
    for (i, n) in notes.iter().enumerate() {
        if n.note > 127 {
            return Err(InvalidNotes(format!(
                "note {i}: note {} is above 127",
                n.note
            )));
        }
        if n.velocity == 0 || n.velocity > 127 {
            return Err(InvalidNotes(format!("note {i}: velocity must be 1..=127")));
        }
        if !(n.start_beats.is_finite() && n.start_beats >= 0.0) {
            return Err(InvalidNotes(format!("note {i}: start must be >= 0")));
        }
        if !(n.length_beats.is_finite() && n.length_beats > 0.0) {
            return Err(InvalidNotes(format!("note {i}: length must be > 0")));
        }
        let on = n.start_beats * secs_per_beat;
        let off = (n.start_beats + n.length_beats) * secs_per_beat;
        events.push(Event {
            at_secs: on,
            note: n.note,
            velocity: n.velocity,
        });
        events.push(Event {
            at_secs: off,
            note: n.note,
            velocity: 0,
        });
    }
    events.sort_by(|a, b| {
        a.at_secs
            .total_cmp(&b.at_secs)
            .then(a.velocity.cmp(&b.velocity))
    });
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(note: u8, start: f64, len: f64) -> Note {
        Note {
            note,
            start_beats: start,
            length_beats: len,
            velocity: 100,
        }
    }

    #[test]
    fn orders_events_and_scales_by_tempo() {
        let ev = plan(&[note(64, 1.0, 1.0), note(60, 0.0, 1.0)], 120.0).expect("plan");
        let got: Vec<(u8, u8, f64)> = ev.iter().map(|e| (e.note, e.velocity, e.at_secs)).collect();
        // 120 bpm = 0.5 s per beat; the off of 60 and the on of 64 coincide at 0.5 s: off first.
        assert_eq!(
            got,
            vec![(60, 100, 0.0), (60, 0, 0.5), (64, 100, 0.5), (64, 0, 1.0)]
        );
    }

    #[test]
    fn empty_list_is_fine() {
        assert_eq!(plan(&[], 120.0), Ok(Vec::new()));
    }

    #[test]
    fn rejects_bad_notes() {
        assert!(plan(&[note(128, 0.0, 1.0)], 120.0).is_err());
        assert!(plan(&[note(60, -1.0, 1.0)], 120.0).is_err());
        assert!(plan(&[note(60, 0.0, 0.0)], 120.0).is_err());
        assert!(
            plan(
                &[Note {
                    velocity: 0,
                    ..note(60, 0.0, 1.0)
                }],
                120.0
            )
            .is_err()
        );
        assert!(plan(&[note(60, f64::NAN, 1.0)], 120.0).is_err());
        assert!(plan(&[note(60, 0.0, 1.0)], 0.0).is_err());
    }
}
