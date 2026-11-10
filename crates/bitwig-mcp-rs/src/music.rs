//! Musical conversions shared by the tools. Pure.
//!
//! * volume in dB <-> Bitwig's normalized fader value,
//! * note names (`C3`, `F#2`, `Bb-1`) <-> MIDI note numbers (Bitwig: C3 = 60),
//! * choosing a note-grid step size for writing notes into a clip.

/// Bitwig's volume fader: amplitude = 2 · x³ for a normalized position x (0 dB at x ≈ 0.794,
/// +6.02 dB at x = 1). The bridge reports Bitwig's own display text next to every value,
/// so a small mismatch with a future Bitwig curve stays visible.
#[must_use]
pub fn db_to_volume(db: f64) -> f64 {
    if db.is_nan() || db == f64::NEG_INFINITY {
        return 0.0;
    }
    let amp = 10f64.powf(db / 20.0);
    (amp / 2.0).cbrt().clamp(0.0, 1.0)
}

/// Inverse of [`db_to_volume`]; 0 gives negative infinity.
#[must_use]
pub fn volume_to_db(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    if x == 0.0 {
        return f64::NEG_INFINITY;
    }
    20.0 * (2.0 * x * x * x).log10()
}

const NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// MIDI note number to a name, Bitwig convention (60 = C3).
#[must_use]
pub fn note_name(note: u8) -> String {
    let octave = i32::from(note / 12) - 2;
    format!("{}{octave}", NAMES[usize::from(note % 12)])
}

/// Parses a note: a number `0..=127` or a name like `C3`, `c#3`, `Db2`, `A-1`.
///
/// # Errors
/// A message naming the bad input.
pub fn parse_note(raw: &str) -> Result<u8, String> {
    let s = raw.trim();
    if let Ok(n) = s.parse::<u8>() {
        return if n <= 127 {
            Ok(n)
        } else {
            Err(format!("note {n} is above 127"))
        };
    }
    let bad = || format!("not a note: {raw:?} (use 0-127 or a name like C3, F#2, Bb1)");
    let mut chars = s.chars();
    let letter = chars.next().ok_or_else(bad)?.to_ascii_uppercase();
    let base: i32 = match letter {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return Err(bad()),
    };
    let rest: String = chars.collect();
    let (accidental, octave) = match rest.chars().next() {
        Some('#') => (1, &rest[1..]),
        Some('b') => (-1, &rest[1..]),
        _ => (0, rest.as_str()),
    };
    let octave: i32 = octave.parse().map_err(|_| bad())?;
    let n = (octave + 2) * 12 + base + accidental;
    u8::try_from(n)
        .ok()
        .filter(|n| *n <= 127)
        .ok_or_else(|| format!("note {raw:?} is outside 0-127"))
}

/// Width of the note grid the bridge observes and writes (steps).
pub const GRID_WIDTH: u32 = 4096;

/// Step sizes tried, coarsest first (beats): straight and triplet divisions.
const STEPS: [f64; 12] = [
    1.0,
    1.0 / 2.0,
    1.0 / 3.0,
    1.0 / 4.0,
    1.0 / 6.0,
    1.0 / 8.0,
    1.0 / 12.0,
    1.0 / 16.0,
    1.0 / 24.0,
    1.0 / 32.0,
    1.0 / 48.0,
    1.0 / 64.0,
];

/// Where notes land on the grid.
#[derive(Debug, Clone, PartialEq)]
pub struct Grid {
    /// Step size in beats.
    pub step: f64,
    /// Step index of every start, in input order.
    pub xs: Vec<u32>,
    /// True when some start was not on the finest grid and got rounded.
    pub rounded: bool,
}

/// Picks the coarsest step that places every start (in beats) exactly on the grid.
/// Falls back to 1/64 beat with rounding.
///
/// # Errors
/// When a start is negative or not finite, or the last note lies beyond the grid.
pub fn choose_grid(starts: &[f64]) -> Result<Grid, String> {
    if let Some(s) = starts.iter().find(|s| !s.is_finite() || **s < 0.0) {
        return Err(format!("note start {s} must be >= 0"));
    }
    let fits = |step: f64| -> Option<Vec<u32>> {
        starts
            .iter()
            .map(|s| {
                let x = s / step;
                ((x - x.round()).abs() < 1e-6 && x.round() < f64::from(GRID_WIDTH))
                    // in range and integral: the cast is exact
                    .then(|| x.round() as u32)
            })
            .collect()
    };
    for step in STEPS {
        if let Some(xs) = fits(step) {
            return Ok(Grid {
                step,
                xs,
                rounded: false,
            });
        }
    }
    let step = STEPS[STEPS.len() - 1];
    let xs: Vec<u32> = starts.iter().map(|s| (s / step).round() as u32).collect();
    if xs.iter().any(|x| *x >= GRID_WIDTH) {
        return Err(format!(
            "notes reach beyond {} beats at 1/64 resolution",
            f64::from(GRID_WIDTH) * step
        ));
    }
    Ok(Grid {
        step,
        xs,
        rounded: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_round_trip() {
        assert!((db_to_volume(0.0) - 0.7937).abs() < 1e-3);
        assert!((db_to_volume(6.0206) - 1.0).abs() < 1e-3);
        assert_eq!(db_to_volume(f64::NEG_INFINITY), 0.0);
        assert_eq!(db_to_volume(40.0), 1.0);
        for db in [-60.0, -12.0, -6.0, -3.5, 0.0, 3.0] {
            assert!((volume_to_db(db_to_volume(db)) - db).abs() < 1e-9, "{db}");
        }
        assert_eq!(volume_to_db(0.0), f64::NEG_INFINITY);
    }

    #[test]
    fn note_names() {
        assert_eq!(parse_note("C3"), Ok(60));
        assert_eq!(parse_note("c#3"), Ok(61));
        assert_eq!(parse_note("Db3"), Ok(61));
        assert_eq!(parse_note("A-1"), Ok(21));
        assert_eq!(parse_note("C-2"), Ok(0));
        assert_eq!(parse_note("G8"), Ok(127));
        assert_eq!(parse_note("64"), Ok(64));
        assert!(parse_note("G#8").is_err());
        assert!(parse_note("200").is_err());
        assert!(parse_note("H2").is_err());
        assert!(parse_note("").is_err());
        assert!(parse_note("C").is_err());
        for n in 0..=127u8 {
            assert_eq!(parse_note(&note_name(n)), Ok(n));
        }
    }

    #[test]
    fn grid_prefers_the_coarsest_exact_step() {
        let g = choose_grid(&[0.0, 1.0, 2.0]).expect("grid");
        assert_eq!((g.step, g.xs, g.rounded), (1.0, vec![0, 1, 2], false));
        let g = choose_grid(&[0.0, 0.25, 0.5]).expect("grid");
        assert_eq!((g.step, g.xs), (0.25, vec![0, 1, 2]));
        let g = choose_grid(&[0.0, 1.0 / 3.0]).expect("grid");
        assert!((g.step - 1.0 / 3.0).abs() < 1e-12);
        let g = choose_grid(&[0.01]).expect("grid");
        assert!(g.rounded);
        assert_eq!(g.xs, vec![1]);
    }

    #[test]
    fn grid_rejects_bad_starts() {
        assert!(choose_grid(&[-1.0]).is_err());
        assert!(choose_grid(&[f64::NAN]).is_err());
        assert!(choose_grid(&[100.01]).is_err());
        assert_eq!(choose_grid(&[]).map(|g| g.xs), Ok(vec![]));
    }
}
