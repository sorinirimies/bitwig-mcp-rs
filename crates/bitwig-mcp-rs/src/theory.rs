//! Music theory for composing: scales, chords, progressions, voicings, rhythms, and the
//! generators and transforms behind `write_chords`, `write_drums`, `write_bassline`,
//! `write_arp` and `transform_clip`. Pure and deterministic (random choices take a seed).
//!
//! Pitch: MIDI numbers, Bitwig naming (60 = C3). Time: beats (quarter notes).

use crate::midi::Note;

/// Pitch class of a note name ("C", "F#", "Bb").
fn pitch_class(name: &str) -> Option<(u8, usize)> {
    let mut chars = name.chars();
    let base: i32 = match chars.next()?.to_ascii_uppercase() {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let rest = &name[1..];
    let (acc, used) = if rest.starts_with('#') || rest.starts_with('♯') {
        (1, rest.chars().next().map_or(0, char::len_utf8))
    } else if rest.starts_with('b') || rest.starts_with('♭') {
        (-1, rest.chars().next().map_or(0, char::len_utf8))
    } else {
        (0, 0)
    };
    Some((((base + acc).rem_euclid(12)) as u8, 1 + used))
}

// ================================================================ scales

/// A key: a root pitch class and the scale's intervals from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scale {
    /// Root pitch class 0-11 (0 = C).
    pub root: u8,
    /// Semitones from the root, ascending, starting with 0.
    pub steps: Vec<u8>,
    /// Name as parsed, e.g. "A minor".
    pub name: String,
}

const SCALES: [(&str, &[u8]); 16] = [
    ("major", &[0, 2, 4, 5, 7, 9, 11]),
    ("ionian", &[0, 2, 4, 5, 7, 9, 11]),
    ("minor", &[0, 2, 3, 5, 7, 8, 10]),
    ("natural minor", &[0, 2, 3, 5, 7, 8, 10]),
    ("aeolian", &[0, 2, 3, 5, 7, 8, 10]),
    ("harmonic minor", &[0, 2, 3, 5, 7, 8, 11]),
    ("melodic minor", &[0, 2, 3, 5, 7, 9, 11]),
    ("dorian", &[0, 2, 3, 5, 7, 9, 10]),
    ("phrygian", &[0, 1, 3, 5, 7, 8, 10]),
    ("lydian", &[0, 2, 4, 6, 7, 9, 11]),
    ("mixolydian", &[0, 2, 4, 5, 7, 9, 10]),
    ("locrian", &[0, 1, 3, 5, 6, 8, 10]),
    ("major pentatonic", &[0, 2, 4, 7, 9]),
    ("minor pentatonic", &[0, 3, 5, 7, 10]),
    ("blues", &[0, 3, 5, 6, 7, 10]),
    ("chromatic", &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]),
];

impl Scale {
    /// Parses "A minor", "C# dorian", "Eb major pentatonic", "Am" (= A minor), "F" (= F major).
    ///
    /// # Errors
    /// Unknown root or scale, with the known scale names.
    pub fn parse(text: &str) -> Result<Self, String> {
        let t = text.trim();
        let (root, used) = pitch_class(t)
            .ok_or_else(|| format!("{text:?}: a key starts with a note, e.g. \"A minor\""))?;
        let rest = t[used..].trim().to_lowercase();
        let rest = match rest.as_str() {
            "" | "maj" => "major",
            "m" | "min" => "minor",
            other => other,
        };
        let steps = SCALES
            .iter()
            .find(|(n, _)| *n == rest)
            .map(|(_, s)| s.to_vec())
            .ok_or_else(|| {
                let names: Vec<&str> = SCALES.iter().map(|(n, _)| *n).collect();
                format!("unknown scale {rest:?}; one of {names:?}")
            })?;
        Ok(Self {
            root,
            steps,
            name: t.to_owned(),
        })
    }

    /// Whether a MIDI note is in the scale.
    #[must_use]
    pub fn contains(&self, note: u8) -> bool {
        let pc = (i32::from(note) - i32::from(self.root)).rem_euclid(12) as u8;
        self.steps.contains(&pc)
    }

    /// The nearest scale note (ties go down).
    #[must_use]
    pub fn snap(&self, note: u8) -> u8 {
        for d in 0..12i32 {
            for cand in [i32::from(note) - d, i32::from(note) + d] {
                if (0..=127).contains(&cand) && self.contains(cand as u8) {
                    return cand as u8;
                }
            }
        }
        note
    }

    /// The triad or seventh built on scale degree `degree` (0-based), stacking scale thirds.
    fn diatonic(&self, degree: usize, seventh: bool) -> Vec<i8> {
        let n = self.steps.len();
        let tone = |k: usize| -> i32 {
            let idx = degree + k;
            i32::from(self.steps[idx % n]) + 12 * (idx / n) as i32
        };
        let base = tone(0);
        let mut out = vec![0, (tone(2) - base) as i8, (tone(4) - base) as i8];
        if seventh {
            out.push((tone(6) - base) as i8);
        }
        out
    }
}

// ================================================================ chords

/// A chord: a root pitch class, intervals above it and an optional bass pitch class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chord {
    /// Root pitch class 0-11.
    pub root: u8,
    /// Semitones above the root, starting with 0.
    pub intervals: Vec<i8>,
    /// Slash-chord bass pitch class.
    pub bass: Option<u8>,
    /// The symbol as written.
    pub symbol: String,
}

/// Chord qualities by suffix, longest first so "maj7" wins over "maj".
const QUALITIES: [(&str, &[i8]); 30] = [
    ("maj13", &[0, 4, 7, 11, 14, 21]),
    ("maj9", &[0, 4, 7, 11, 14]),
    ("maj7", &[0, 4, 7, 11]),
    ("mmaj7", &[0, 3, 7, 11]),
    ("m7b5", &[0, 3, 6, 10]),
    ("min7", &[0, 3, 7, 10]),
    ("m13", &[0, 3, 7, 10, 14, 21]),
    ("m11", &[0, 3, 7, 10, 14, 17]),
    ("m9", &[0, 3, 7, 10, 14]),
    ("m7", &[0, 3, 7, 10]),
    ("m6", &[0, 3, 7, 9]),
    ("dim7", &[0, 3, 6, 9]),
    ("dim", &[0, 3, 6]),
    ("aug", &[0, 4, 8]),
    ("sus2", &[0, 2, 7]),
    ("sus4", &[0, 5, 7]),
    ("add9", &[0, 4, 7, 14]),
    ("13", &[0, 4, 7, 10, 14, 21]),
    ("11", &[0, 4, 7, 10, 14, 17]),
    ("9", &[0, 4, 7, 10, 14]),
    ("7", &[0, 4, 7, 10]),
    ("6", &[0, 4, 7, 9]),
    ("5", &[0, 7]),
    ("maj", &[0, 4, 7]),
    ("min", &[0, 3, 7]),
    ("ø", &[0, 3, 6, 10]),
    ("°", &[0, 3, 6]),
    ("+", &[0, 4, 8]),
    ("m", &[0, 3, 7]),
    ("", &[0, 4, 7]),
];

impl Chord {
    /// Parses a chord symbol: "Am7", "Fmaj9", "C/E", "Bbsus4", "F#m7b5", "G7".
    ///
    /// # Errors
    /// Unknown roots or qualities.
    pub fn parse(symbol: &str) -> Result<Self, String> {
        let s = symbol.trim();
        let (main, bass) = match s.split_once('/') {
            Some((m, b)) => {
                let (pc, used) =
                    pitch_class(b.trim()).ok_or_else(|| format!("bad bass note in {s:?}"))?;
                if used != b.trim().len() {
                    return Err(format!("bad bass note in {s:?}"));
                }
                (m, Some(pc))
            }
            None => (s, None),
        };
        let (root, used) = pitch_class(main).ok_or_else(|| format!("not a chord: {s:?}"))?;
        let suffix = &main[used..];
        let intervals = QUALITIES
            .iter()
            .find(|(q, _)| *q == suffix)
            .map(|(_, i)| i.to_vec())
            .ok_or_else(|| format!("unknown chord quality {suffix:?} in {s:?}"))?;
        Ok(Self {
            root,
            intervals,
            bass,
            symbol: s.to_owned(),
        })
    }

    /// Parses a Roman numeral in a key: "i", "VI", "ii7", "V7", "bVII", "iv", "viio", "Imaj7".
    /// Upper case = major, lower case = minor (unless the key already fixes the quality through
    /// the scale, in which case the case is still honoured).
    ///
    /// # Errors
    /// Unknown numerals or suffixes.
    pub fn roman(token: &str, key: &Scale) -> Result<Self, String> {
        let t = token.trim();
        let (shift, body) = match t.chars().next() {
            Some('b' | '♭') => (-1, &t[t.chars().next().map_or(1, char::len_utf8)..]),
            Some('#' | '♯') => (1, &t[t.chars().next().map_or(1, char::len_utf8)..]),
            _ => (0, t),
        };
        let numeral_len = body
            .chars()
            .take_while(|c| matches!(c.to_ascii_uppercase(), 'I' | 'V'))
            .count();
        if numeral_len == 0 {
            return Err(format!("not a Roman numeral: {t:?}"));
        }
        let numeral = &body[..numeral_len];
        let degree = match numeral.to_ascii_uppercase().as_str() {
            "I" => 0,
            "II" => 1,
            "III" => 2,
            "IV" => 3,
            "V" => 4,
            "VI" => 5,
            "VII" => 6,
            _ => return Err(format!("not a Roman numeral: {t:?}")),
        };
        let upper = numeral.chars().all(|c| c.is_ascii_uppercase());
        let suffix = &body[numeral_len..];
        let n = key.steps.len();
        if n != 7 {
            return Err("Roman numerals need a seven-note scale (major, minor, modes)".into());
        }
        let root =
            ((i32::from(key.root) + i32::from(key.steps[degree]) + shift).rem_euclid(12)) as u8;
        let intervals: Vec<i8> = match suffix {
            "" => {
                let third = if upper { 4 } else { 3 };
                let fifth = key.diatonic(degree, false)[2];
                // keep the scale's diminished fifth on vii° etc. only when the case agrees
                let fifth = if (fifth == 6 && !upper) || (fifth == 8 && upper) {
                    fifth
                } else {
                    7
                };
                vec![0, third, fifth]
            }
            "7" => {
                let mut d = key.diatonic(degree, true);
                d[1] = if upper { 4 } else { 3 };
                d
            }
            "o" | "°" | "dim" => vec![0, 3, 6],
            "o7" | "°7" | "dim7" => vec![0, 3, 6, 9],
            "ø" | "ø7" | "m7b5" => vec![0, 3, 6, 10],
            "+" | "aug" => vec![0, 4, 8],
            other => {
                // a chord-symbol suffix on the numeral's root: "Imaj7", "iim9", "Vsus4"
                let q = if upper || other.starts_with('m') {
                    other.to_owned()
                } else {
                    format!("m{other}")
                };
                QUALITIES
                    .iter()
                    .find(|(name, _)| *name == q)
                    .map(|(_, i)| i.to_vec())
                    .ok_or_else(|| format!("unknown suffix {other:?} in {t:?}"))?
            }
        };
        Ok(Self {
            root,
            intervals,
            bass: None,
            symbol: t.to_owned(),
        })
    }

    /// Chord tones as pitch classes in stacking order.
    fn classes(&self) -> Vec<u8> {
        self.intervals
            .iter()
            .map(|i| ((i32::from(self.root) + i32::from(*i)).rem_euclid(12)) as u8)
            .collect()
    }
}

/// Parses a progression: chord symbols ("Am F C G") or Roman numerals in `key` ("i VI III VII"),
/// separated by spaces, commas, dashes or bars.
///
/// # Errors
/// The first token that is neither.
pub fn progression(text: &str, key: Option<&Scale>) -> Result<Vec<Chord>, String> {
    let tokens: Vec<&str> = text
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | '|' | '–' | '—'))
        .flat_map(|t| t.split(" - "))
        .map(str::trim)
        .filter(|t| !t.is_empty() && *t != "-")
        .collect();
    if tokens.is_empty() {
        return Err("the progression is empty".into());
    }
    tokens
        .iter()
        .map(|t| {
            let first = t.trim_start_matches(['b', '#', '♭', '♯']).chars().next();
            let looks_roman = matches!(first.map(|c| c.to_ascii_uppercase()), Some('I' | 'V'));
            match (looks_roman, key) {
                (true, Some(k)) => Chord::roman(t, k),
                (true, None) => Err(format!("{t:?} is a Roman numeral: give a key")),
                (false, _) => Chord::parse(t),
            }
        })
        .collect()
}

// ================================================================ voicing

/// How chord tones are spread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Voicing {
    /// Tones as close as possible, moving smoothly from the previous chord.
    #[default]
    Close,
    /// Root low, the rest an octave up (wider, pad-like).
    Open,
    /// Drop-2: the second-highest close tone dropped an octave.
    Drop2,
    /// Root position, stacked in order (no voice leading).
    Root,
}

/// MIDI note of pitch class `pc` in Bitwig octave `octave` (C3 = 60).
fn note_in(pc: u8, octave: i32) -> i32 {
    (octave + 2) * 12 + i32::from(pc)
}

fn clamp_note(n: i32) -> u8 {
    n.clamp(0, 127) as u8
}

/// Voices a chord around `octave`. With [`Voicing::Close`] and a previous voicing, picks the
/// inversion closest to it (smallest total movement).
#[must_use]
pub fn voice(chord: &Chord, octave: i32, voicing: Voicing, prev: Option<&[u8]>) -> Vec<u8> {
    let root = note_in(chord.root, octave);
    let stacked: Vec<i32> = chord
        .intervals
        .iter()
        .map(|i| root + i32::from(*i))
        .collect();
    let mut notes = match voicing {
        Voicing::Root => stacked,
        Voicing::Open => {
            let mut v = vec![stacked[0] - 12];
            v.extend(stacked.iter().skip(1));
            v
        }
        Voicing::Drop2 => {
            let mut v = close(&chord.classes(), root);
            if v.len() >= 3 {
                let i = v.len() - 2;
                v[i] -= 12;
            }
            v
        }
        Voicing::Close => {
            let classes = chord.classes();
            let candidates: Vec<Vec<i32>> = (-1..=1)
                .flat_map(|oct| (0..classes.len()).map(move |inv| (oct, inv)))
                .map(|(oct, inv)| {
                    let mut rotated = classes.clone();
                    rotated.rotate_left(inv);
                    close(&rotated, note_in(rotated[0], octave + oct))
                })
                .collect();
            match prev.filter(|p| !p.is_empty()) {
                Some(p) => {
                    let centre: f64 = p.iter().map(|n| f64::from(*n)).sum::<f64>() / p.len() as f64;
                    candidates
                        .into_iter()
                        .min_by(|a, b| {
                            let cost = |v: &Vec<i32>| {
                                let c =
                                    v.iter().map(|n| f64::from(*n)).sum::<f64>() / v.len() as f64;
                                (c - centre).abs()
                            };
                            cost(a).total_cmp(&cost(b))
                        })
                        .unwrap_or_default()
                }
                None => close(&classes, root),
            }
        }
    };
    if let Some(b) = chord.bass {
        let lowest = notes.iter().copied().min().unwrap_or(root);
        let mut bass = note_in(b, octave - 1);
        while bass >= lowest {
            bass -= 12;
        }
        notes.insert(0, bass);
    }
    notes.sort_unstable();
    notes.dedup();
    notes.into_iter().map(clamp_note).collect()
}

/// Stacks pitch classes upward from `start` (which has the first class), each above the last.
fn close(classes: &[u8], start: i32) -> Vec<i32> {
    let mut out = vec![start];
    for pc in classes.iter().skip(1) {
        let last = *out.last().unwrap_or(&start);
        let mut n = last - (last.rem_euclid(12)) + i32::from(*pc);
        while n <= last {
            n += 12;
        }
        out.push(n);
    }
    out
}

// ================================================================ rhythm

/// One step of a pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Rest.
    Rest,
    /// Hit.
    Hit,
    /// Accented hit.
    Accent,
    /// Holds the previous hit one more step.
    Tie,
}

/// Parses a step pattern: `x` hit, `X` accent, `.` or `-` rest, `_` tie (extends the previous
/// hit); spaces and `|` are ignored.
///
/// # Errors
/// Unknown characters or an empty pattern.
pub fn steps(pattern: &str) -> Result<Vec<Step>, String> {
    let out: Vec<Step> = pattern
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '|')
        .map(|c| match c {
            'x' | 'o' | '1' => Ok(Step::Hit),
            'X' | 'O' => Ok(Step::Accent),
            '.' | '-' | '0' => Ok(Step::Rest),
            '_' => Ok(Step::Tie),
            other => Err(format!(
                "unknown step {other:?} in {pattern:?} (use x X . _)"
            )),
        })
        .collect::<Result<_, _>>()?;
    if out.is_empty() {
        return Err("empty pattern".into());
    }
    Ok(out)
}

/// A Euclidean rhythm: `hits` spread as evenly as possible over `steps`, rotated right.
#[must_use]
pub fn euclid(hits: usize, steps: usize, rotate: usize) -> Vec<Step> {
    if steps == 0 {
        return Vec::new();
    }
    let hits = hits.min(steps);
    let mut out: Vec<Step> = (0..steps)
        .map(|i| {
            if (i * hits) % steps < hits {
                Step::Hit
            } else {
                Step::Rest
            }
        })
        .collect();
    out.rotate_right(rotate % steps);
    out
}

/// Turns steps into (start, length, accent) in beats, repeating the pattern to `length` beats.
#[must_use]
pub fn hits(pattern: &[Step], step_beats: f64, length: f64) -> Vec<(f64, f64, bool)> {
    let mut out: Vec<(f64, f64, bool)> = Vec::new();
    if pattern.is_empty() || step_beats <= 0.0 {
        return out;
    }
    let total = (length / step_beats).round() as usize;
    for i in 0..total {
        let start = i as f64 * step_beats;
        match pattern[i % pattern.len()] {
            Step::Hit => out.push((start, step_beats, false)),
            Step::Accent => out.push((start, step_beats, true)),
            Step::Tie => {
                if let Some(last) = out.last_mut() {
                    last.1 += step_beats;
                }
            }
            Step::Rest => {}
        }
    }
    out
}

// ================================================================ generators

/// Chord notes: each chord lasts `beats_per_chord`; `rhythm` (steps of `step_beats`) re-strikes
/// it, else it is held. Close voicing moves smoothly between chords.
#[must_use]
pub fn chord_notes(
    chords: &[Chord],
    beats_per_chord: f64,
    octave: i32,
    voicing: Voicing,
    rhythm: Option<(&[Step], f64)>,
    velocity: u8,
) -> Vec<Note> {
    let mut out = Vec::new();
    let mut prev: Option<Vec<u8>> = None;
    for (i, chord) in chords.iter().enumerate() {
        let start = i as f64 * beats_per_chord;
        let tones = voice(chord, octave, voicing, prev.as_deref());
        let strikes = match rhythm {
            Some((pattern, step)) => hits(pattern, step, beats_per_chord),
            None => vec![(0.0, beats_per_chord, false)],
        };
        for (s, len, accent) in strikes {
            for t in &tones {
                out.push(Note {
                    note: *t,
                    start_beats: start + s,
                    length_beats: len,
                    velocity: accented(velocity, accent),
                });
            }
        }
        prev = Some(tones);
    }
    out
}

fn accented(velocity: u8, accent: bool) -> u8 {
    if accent {
        velocity.saturating_add(25).min(127)
    } else {
        velocity.clamp(1, 127)
    }
}

/// How a bassline follows the chords.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BassStyle {
    /// The root on every hit.
    #[default]
    Root,
    /// Root and octave alternating.
    Octave,
    /// Root and fifth alternating.
    Fifth,
    /// Root, third, fifth, octave cycling.
    Arpeggio,
    /// Steps toward the next chord's root on the last hit.
    Walking,
}

/// A bassline over the chords. `rhythm` (steps of `step_beats`) sets the hits per chord;
/// the default is eighth notes.
#[must_use]
pub fn bassline(
    chords: &[Chord],
    beats_per_chord: f64,
    octave: i32,
    style: BassStyle,
    rhythm: Option<(&[Step], f64)>,
    velocity: u8,
) -> Vec<Note> {
    let default = [Step::Hit, Step::Rest, Step::Hit, Step::Hit];
    let (pattern, step) = rhythm.unwrap_or((&default, 0.5));
    let mut out = Vec::new();
    for (ci, chord) in chords.iter().enumerate() {
        let start = ci as f64 * beats_per_chord;
        let root = note_in(chord.bass.unwrap_or(chord.root), octave);
        let third = root + i32::from(*chord.intervals.get(1).unwrap_or(&4));
        let fifth = root + i32::from(*chord.intervals.get(2).unwrap_or(&7));
        let cycle = [root, third, fifth, root + 12];
        let next_root = chords
            .get(ci + 1)
            .map(|c| note_in(c.bass.unwrap_or(c.root), octave));
        let strikes = hits(pattern, step, beats_per_chord);
        let last = strikes.len().saturating_sub(1);
        for (k, (s, len, accent)) in strikes.into_iter().enumerate() {
            let n = match style {
                BassStyle::Root => root,
                BassStyle::Octave => {
                    if k % 2 == 0 {
                        root
                    } else {
                        root + 12
                    }
                }
                BassStyle::Fifth => {
                    if k % 2 == 0 {
                        root
                    } else {
                        fifth
                    }
                }
                BassStyle::Arpeggio => cycle[k % cycle.len()],
                BassStyle::Walking => match next_root {
                    Some(nr) if k == last && k > 0 => {
                        if nr > root {
                            nr - 1
                        } else {
                            nr + 1
                        }
                    }
                    _ => cycle[k % 3],
                },
            };
            out.push(Note {
                note: clamp_note(n),
                start_beats: start + s,
                length_beats: len * 0.9,
                velocity: accented(velocity, accent),
            });
        }
    }
    out
}

/// Arpeggio direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArpMode {
    /// Low to high.
    #[default]
    Up,
    /// High to low.
    Down,
    /// Up then down, without repeating the ends.
    UpDown,
    /// Seeded random order.
    Random,
}

/// Arpeggiates each chord's voicing over `octaves` at `rate` beats per note.
// each argument is a musical choice the tool exposes; a struct would only rename them
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn arpeggio(
    chords: &[Chord],
    beats_per_chord: f64,
    octave: i32,
    mode: ArpMode,
    rate: f64,
    octaves: u8,
    gate: f64,
    velocity: u8,
    seed: u64,
) -> Vec<Note> {
    let mut rng = Rng::new(seed);
    let mut out = Vec::new();
    let mut prev: Option<Vec<u8>> = None;
    for (ci, chord) in chords.iter().enumerate() {
        let base = voice(chord, octave, Voicing::Close, prev.as_deref());
        let mut tones: Vec<i32> = Vec::new();
        for o in 0..i32::from(octaves.max(1)) {
            tones.extend(base.iter().map(|n| i32::from(*n) + 12 * o));
        }
        let order: Vec<i32> = match mode {
            ArpMode::Up => tones.clone(),
            ArpMode::Down => tones.iter().rev().copied().collect(),
            ArpMode::UpDown => {
                let mut v = tones.clone();
                v.extend(
                    tones
                        .iter()
                        .rev()
                        .skip(1)
                        .take(tones.len().saturating_sub(2)),
                );
                v
            }
            ArpMode::Random => tones.clone(),
        };
        let count = (beats_per_chord / rate).round().max(1.0) as usize;
        for k in 0..count {
            let n = match mode {
                ArpMode::Random => order[rng.below(order.len())],
                _ => order[k % order.len()],
            };
            out.push(Note {
                note: clamp_note(n),
                start_beats: ci as f64 * beats_per_chord + k as f64 * rate,
                length_beats: rate * gate.clamp(0.05, 1.0),
                velocity: velocity.clamp(1, 127),
            });
        }
        prev = Some(base);
    }
    out
}

/// Drum hits: `voices` maps a MIDI key to its steps; repeats each to `length` beats.
#[must_use]
pub fn drums(voices: &[(u8, Vec<Step>)], step_beats: f64, length: f64, velocity: u8) -> Vec<Note> {
    let mut out = Vec::new();
    for (key, pattern) in voices {
        for (s, _, accent) in hits(pattern, step_beats, length) {
            out.push(Note {
                note: *key,
                start_beats: s,
                // short, so a sampler retriggers; drum devices ignore the length anyway
                length_beats: step_beats * 0.5,
                velocity: accented(velocity, accent),
            });
        }
    }
    out.sort_by(|a, b| {
        a.start_beats
            .total_cmp(&b.start_beats)
            .then(a.note.cmp(&b.note))
    });
    out
}

// ================================================================ melody

/// The overall shape of a melodic phrase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Contour {
    /// Rises to a peak in the middle, then falls (default).
    #[default]
    Arch,
    /// Climbs from low to high.
    Rise,
    /// Falls from high to low.
    Fall,
    /// Up, down, up: a wave.
    Wave,
    /// Stays around the middle.
    Flat,
}

impl Contour {
    /// Target height 0-1 at phrase position `t` (0-1).
    fn height(self, t: f64) -> f64 {
        match self {
            Self::Arch => 1.0 - (2.0 * t - 1.0).abs(),
            Self::Rise => t,
            Self::Fall => 1.0 - t,
            Self::Wave => 0.5 + 0.5 * (t * std::f64::consts::TAU).sin(),
            Self::Flat => 0.5,
        }
    }
}

/// What shapes a melody; see `write_melody`.
#[derive(Debug, Clone, PartialEq)]
pub struct MelodySpec {
    /// Beats each chord lasts.
    pub beats_per_chord: f64,
    /// Bitwig octave of the lowest notes.
    pub octave: i32,
    /// Octaves the melody spans (1-3).
    pub span: u8,
    /// Phrase shape.
    pub contour: Contour,
    /// Hits per step, 0-1.
    pub density: f64,
    /// Smallest step in beats.
    pub rate: f64,
    /// The motif's length in beats; it repeats to fill the clip.
    pub motif_beats: f64,
    /// Rhythm of the motif as steps (overrides density).
    pub rhythm: Option<Vec<Step>>,
    /// How much each repetition changes, 0-1.
    pub variation: f64,
    /// Velocity.
    pub velocity: u8,
    /// Seed.
    pub seed: u64,
}

/// One motif event: step in the motif, scale-pool index, length in steps.
type MotifNote = (usize, usize, usize);

/// A melody in `scale` over `chords`: a short motif (a rhythm and a contour) repeated through
/// the clip with small changes. Notes on a chord change and the last note are chord tones; all
/// others are scale notes, moving mostly by step.
#[must_use]
pub fn melody(scale: &Scale, chords: &[Chord], spec: &MelodySpec) -> Vec<Note> {
    let mut rng = Rng::new(spec.seed);
    let total = spec.beats_per_chord * chords.len() as f64;
    let step = spec.rate.max(0.0625);
    let motif_steps = ((spec.motif_beats / step).round() as usize).max(2);
    let low = note_in(scale.root, spec.octave);
    let high = low + 12 * i32::from(spec.span.clamp(1, 3));
    let pool: Vec<i32> = (low..=high)
        .filter(|n| scale.contains(clamp_note(*n)))
        .collect();
    if pool.is_empty() || chords.is_empty() {
        return Vec::new();
    }
    let chord_tone = |chord: &Chord, idx: usize| -> usize {
        // nearest pool index to `idx` whose pitch is in the chord
        let pcs: Vec<i32> = chord
            .intervals
            .iter()
            .map(|i| (i32::from(chord.root) + i32::from(*i)).rem_euclid(12))
            .collect();
        (0..pool.len())
            .filter(|k| pcs.contains(&pool[*k].rem_euclid(12)))
            .min_by_key(|k| k.abs_diff(idx))
            .unwrap_or(idx)
    };

    // rhythm of the motif: hit positions with lengths in steps
    let mut hit_steps: Vec<usize> = match &spec.rhythm {
        Some(pattern) => pattern
            .iter()
            .take(motif_steps)
            .enumerate()
            .filter(|(_, st)| !matches!(st, Step::Rest | Step::Tie))
            .map(|(i, _)| i)
            .collect(),
        None => (0..motif_steps)
            .filter(|i| {
                let strong = i % 4 == 0;
                *i == 0
                    || rng.signed().mul_add(0.5, 0.5)
                        < spec.density * if strong { 1.3 } else { 0.9 }
            })
            .collect(),
    };
    if hit_steps.len() < 2 {
        hit_steps = vec![0, motif_steps / 2];
    }

    // pitches of the motif: follow the contour, mostly by step
    let first = chord_tone(&chords[0], pool.len() / 3);
    let mut prev = first;
    let motif: Vec<MotifNote> = hit_steps
        .iter()
        .enumerate()
        .map(|(k, st)| {
            let t = k as f64 / (hit_steps.len().max(2) - 1) as f64;
            let target = (spec.contour.height(t) * (pool.len() - 1) as f64).round() as i64;
            let toward = (target - prev as i64).clamp(-2, 2);
            let wobble = rng.below(3) as i64 - 1;
            let idx = (prev as i64 + toward + wobble).clamp(0, pool.len() as i64 - 1) as usize;
            prev = idx;
            let next = hit_steps.get(k + 1).copied().unwrap_or(motif_steps);
            (*st, if k == 0 { first } else { idx }, next - st)
        })
        .collect();

    // repeat through the clip, varying; the first note of each chord is a chord tone
    let reps = (total / spec.motif_beats).ceil().max(1.0) as usize;
    let mut out = Vec::new();
    for rep in 0..reps {
        let mut m = motif.clone();
        if rep > 0 && spec.variation > 0.0 {
            let changes = ((m.len() as f64) * spec.variation).round().max(1.0) as usize;
            for _ in 0..changes {
                let k = 1 + rng.below(m.len().saturating_sub(1).max(1));
                if let Some(n) = m.get_mut(k) {
                    let delta = if rng.below(2) == 0 { -1 } else { 1 } * (1 + rng.below(2) as i64);
                    n.1 = (n.1 as i64 + delta).clamp(0, pool.len() as i64 - 1) as usize;
                }
            }
        }
        let last_rep = rep + 1 == reps;
        let count = m.len();
        for (k, (st, idx, len)) in m.into_iter().enumerate() {
            let start = rep as f64 * spec.motif_beats + st as f64 * step;
            if start >= total {
                continue;
            }
            let chord = &chords[((start / spec.beats_per_chord) as usize).min(chords.len() - 1)];
            let on_chord_start = start % spec.beats_per_chord < step * 0.99;
            let idx = if on_chord_start || (last_rep && k + 1 == count) {
                chord_tone(chord, idx)
            } else {
                idx
            };
            let end = (start + len as f64 * step * 0.9).min(total);
            out.push(Note {
                note: clamp_note(pool[idx]),
                start_beats: start,
                length_beats: (end - start).max(step * 0.25),
                velocity: accented(spec.velocity, st % 4 == 0),
            });
        }
    }
    out
}

// ================================================================ transforms

/// A change to existing notes.
#[derive(Debug, Clone, PartialEq, serde::Deserialize, schemars::JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Transform {
    /// Shift every note by semitones.
    Transpose {
        /// Semitones, e.g. 12 = one octave up.
        semitones: i32,
    },
    /// Move every note to the nearest note of a key, e.g. "A minor".
    ToScale {
        /// Key.
        key: String,
    },
    /// Random timing and velocity offsets (seeded, so repeatable).
    Humanize {
        /// Max timing offset in beats (default 0.02).
        timing: Option<f64>,
        /// Max velocity offset (default 10).
        velocity: Option<u8>,
        /// Seed (default 1).
        seed: Option<u64>,
    },
    /// Pull starts toward a grid.
    Quantize {
        /// Grid in beats, e.g. 0.25 for 1/16.
        grid: f64,
        /// 0-1 (default 1 = fully).
        strength: Option<f64>,
    },
    /// Mirror in time within the clip.
    Reverse,
    /// Multiply times: 0.5 = double time, 2 = half time.
    Stretch {
        /// Factor.
        factor: f64,
    },
    /// Scale and offset velocities.
    Velocity {
        /// Multiply (default 1).
        scale: Option<f64>,
        /// Add (default 0).
        offset: Option<i32>,
    },
    /// Keep only every n-th note (thin out), or drop notes below a velocity.
    Thin {
        /// Keep one in `every` notes.
        every: Option<usize>,
        /// Drop notes softer than this.
        min_velocity: Option<u8>,
    },
}

/// Applies transforms in order. `length` is the clip length in beats (for reverse and to keep
/// notes inside the clip).
///
/// # Errors
/// Bad keys or factors.
pub fn transform(notes: &[Note], ops: &[Transform], length: f64) -> Result<Vec<Note>, String> {
    let mut out = notes.to_vec();
    for op in ops {
        match op {
            Transform::Transpose { semitones } => {
                for n in &mut out {
                    n.note = clamp_note(i32::from(n.note) + semitones);
                }
            }
            Transform::ToScale { key } => {
                let scale = Scale::parse(key)?;
                for n in &mut out {
                    n.note = scale.snap(n.note);
                }
            }
            Transform::Humanize {
                timing,
                velocity,
                seed,
            } => {
                let mut rng = Rng::new(seed.unwrap_or(1));
                let t = timing.unwrap_or(0.02).abs();
                let v = i32::from(velocity.unwrap_or(10));
                for n in &mut out {
                    n.start_beats = (n.start_beats + rng.signed() * t).max(0.0);
                    let dv = (rng.signed() * f64::from(v)).round() as i32;
                    n.velocity = (i32::from(n.velocity) + dv).clamp(1, 127) as u8;
                }
            }
            Transform::Quantize { grid, strength } => {
                if !(grid.is_finite() && *grid > 0.0) {
                    return Err("grid must be > 0 beats".into());
                }
                let k = strength.unwrap_or(1.0).clamp(0.0, 1.0);
                for n in &mut out {
                    let target = (n.start_beats / grid).round() * grid;
                    n.start_beats += (target - n.start_beats) * k;
                }
            }
            Transform::Reverse => {
                for n in &mut out {
                    n.start_beats = (length - n.start_beats - n.length_beats).max(0.0);
                }
            }
            Transform::Stretch { factor } => {
                if !(factor.is_finite() && *factor > 0.0) {
                    return Err("factor must be > 0".into());
                }
                for n in &mut out {
                    n.start_beats *= factor;
                    n.length_beats *= factor;
                }
            }
            Transform::Velocity { scale, offset } => {
                let s = scale.unwrap_or(1.0);
                let o = offset.unwrap_or(0);
                for n in &mut out {
                    let v = (f64::from(n.velocity) * s).round() as i32 + o;
                    n.velocity = v.clamp(1, 127) as u8;
                }
            }
            Transform::Thin {
                every,
                min_velocity,
            } => {
                let every = every.unwrap_or(1).max(1);
                let min_v = min_velocity.unwrap_or(0);
                out.sort_by(|a, b| a.start_beats.total_cmp(&b.start_beats));
                out = out
                    .into_iter()
                    .enumerate()
                    .filter(|(i, n)| i % every == 0 && n.velocity >= min_v)
                    .map(|(_, n)| n)
                    .collect();
            }
        }
    }
    // keep everything playable inside the clip
    out.retain(|n| n.start_beats < length);
    for n in &mut out {
        n.length_beats = n.length_beats.min(length - n.start_beats).max(1.0 / 64.0);
    }
    out.sort_by(|a, b| {
        a.start_beats
            .total_cmp(&b.start_beats)
            .then(a.note.cmp(&b.note))
    });
    Ok(out)
}

/// Small deterministic generator (xorshift64*), so seeds reproduce.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    /// Seeded; 0 is replaced by a fixed constant.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self(if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        })
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// In 0..n (n > 0).
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    /// In -1.0..=1.0.
    pub fn signed(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    }
}

#[cfg(test)]
mod tests {
    fn melody_spec() -> MelodySpec {
        MelodySpec {
            beats_per_chord: 4.0,
            octave: 3,
            span: 1,
            contour: Contour::Arch,
            density: 0.6,
            rate: 0.5,
            motif_beats: 4.0,
            rhythm: None,
            variation: 0.3,
            velocity: 90,
            seed: 7,
        }
    }

    #[test]
    fn melody_stays_in_key_starts_on_chord_tones_and_is_repeatable() {
        let scale = Scale::parse("A minor").expect("key");
        let chords = progression("i VI III VII", Some(&scale)).expect("chords");
        let a = melody(&scale, &chords, &melody_spec());
        assert!(a.len() >= 8, "{}", a.len());
        assert!(a.iter().all(|n| scale.contains(n.note)));
        assert!(a.iter().all(|n| n.start_beats < 16.0));
        // first note of each chord is a chord tone
        for (i, chord) in chords.iter().enumerate() {
            let at = i as f64 * 4.0;
            let first = a.iter().find(|n| n.start_beats >= at).expect("note");
            let pcs: Vec<i32> = chord
                .intervals
                .iter()
                .map(|x| (i32::from(chord.root) + i32::from(*x)).rem_euclid(12))
                .collect();
            if first.start_beats < at + 0.5 {
                assert!(
                    pcs.contains(&(i32::from(first.note) % 12)),
                    "{chord:?} {first:?}"
                );
            }
        }
        assert_eq!(
            a,
            melody(&scale, &chords, &melody_spec()),
            "same seed, same melody"
        );
        let mut other = melody_spec();
        other.seed = 8;
        assert_ne!(a, melody(&scale, &chords, &other));
    }

    #[test]
    fn melody_repeats_its_rhythm_with_variation() {
        let scale = Scale::parse("C major").expect("key");
        let chords = progression("C", Some(&scale)).expect("chords");
        let mut spec = melody_spec();
        spec.beats_per_chord = 16.0;
        let a = melody(&scale, &chords, &spec);
        let starts = |from: f64| -> Vec<f64> {
            a.iter()
                .filter(|n| n.start_beats >= from && n.start_beats < from + 4.0)
                .map(|n| n.start_beats - from)
                .collect()
        };
        assert_eq!(starts(0.0), starts(4.0));
        assert_eq!(starts(0.0), starts(8.0));
    }

    use super::*;

    fn pcs(notes: &[u8]) -> Vec<u8> {
        let mut v: Vec<u8> = notes.iter().map(|n| n % 12).collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    #[test]
    fn scales_parse_and_snap() {
        let a = Scale::parse("A minor").expect("a minor");
        assert_eq!((a.root, a.steps.len()), (9, 7));
        assert_eq!(Scale::parse("Am").expect("am").steps, a.steps);
        assert_eq!(Scale::parse("F").expect("f").steps, SCALES[0].1);
        assert_eq!(Scale::parse("Eb dorian").expect("eb").root, 3);
        assert!(Scale::parse("H minor").is_err());
        assert!(Scale::parse("C funky").unwrap_err().contains("dorian"));
        assert!(a.contains(60)); // C
        assert!(!a.contains(61)); // C#
        assert_eq!(a.snap(61), 60);
        assert_eq!(a.snap(66), 65); // F# -> F (tie goes down)
    }

    #[test]
    fn chord_symbols() {
        let am7 = Chord::parse("Am7").expect("am7");
        assert_eq!((am7.root, am7.intervals.clone()), (9, vec![0, 3, 7, 10]));
        assert_eq!(
            Chord::parse("Fmaj9").expect("f").intervals,
            vec![0, 4, 7, 11, 14]
        );
        assert_eq!(Chord::parse("C").expect("c").intervals, vec![0, 4, 7]);
        assert_eq!(Chord::parse("F#m7b5").expect("f#").root, 6);
        let slash = Chord::parse("C/E").expect("c/e");
        assert_eq!(slash.bass, Some(4));
        assert!(Chord::parse("Cxyz").is_err());
        assert!(Chord::parse("C/H").is_err());
    }

    #[test]
    fn roman_numerals_follow_the_key() {
        let a = Scale::parse("A minor").expect("key");
        let prog = progression("i VI III VII", Some(&a)).expect("prog");
        let roots: Vec<u8> = prog.iter().map(|c| c.root).collect();
        assert_eq!(roots, vec![9, 5, 0, 7]); // A F C G
        assert_eq!(prog[0].intervals, vec![0, 3, 7]);
        assert_eq!(prog[1].intervals, vec![0, 4, 7]);
        let c = Scale::parse("C major").expect("c");
        let jazz = progression("ii7 V7 Imaj7", Some(&c)).expect("251");
        assert_eq!(jazz[0].intervals, vec![0, 3, 7, 10]);
        assert_eq!(jazz[1].intervals, vec![0, 4, 7, 10]);
        assert_eq!(jazz[2].intervals, vec![0, 4, 7, 11]);
        assert_eq!(Chord::roman("bVII", &c).expect("bvii").root, 10);
        assert_eq!(
            Chord::roman("viio", &c).expect("vii").intervals,
            vec![0, 3, 6]
        );
        assert!(progression("I IV V", None).is_err());
        assert!(progression("", Some(&c)).is_err());
        assert_eq!(
            progression("Am, F | C - G", None).expect("symbols").len(),
            4
        );
    }

    #[test]
    fn voicings_keep_the_chord_and_move_smoothly() {
        let c = Chord::parse("C").expect("c");
        let close = voice(&c, 3, Voicing::Close, None);
        assert_eq!(close, vec![60, 64, 67]);
        assert_eq!(voice(&c, 3, Voicing::Open, None), vec![48, 64, 67]);
        let f = Chord::parse("F").expect("f");
        let led = voice(&f, 3, Voicing::Close, Some(&close));
        assert_eq!(pcs(&led), vec![0, 5, 9]);
        let centre = |v: &[u8]| v.iter().map(|n| f64::from(*n)).sum::<f64>() / v.len() as f64;
        assert!(
            (centre(&led) - centre(&close)).abs() <= 4.0,
            "{led:?} moves little"
        );
        let slash = voice(&Chord::parse("C/E").expect("c/e"), 3, Voicing::Close, None);
        assert_eq!(slash[0] % 12, 4, "bass note lowest: {slash:?}");
        assert_eq!(
            voice(&Chord::parse("Cmaj7").expect("c"), 3, Voicing::Drop2, None).len(),
            4
        );
    }

    #[test]
    fn steps_and_euclid() {
        assert_eq!(
            steps("x.X_ |.").expect("steps"),
            vec![Step::Hit, Step::Rest, Step::Accent, Step::Tie, Step::Rest]
        );
        assert!(steps("x?").is_err());
        assert!(steps("  ").is_err());
        let tresillo = euclid(3, 8, 0);
        assert_eq!(tresillo.iter().filter(|s| **s == Step::Hit).count(), 3);
        assert_eq!(tresillo[0], Step::Hit);
        assert_eq!(euclid(3, 8, 1)[1], Step::Hit);
        assert!(euclid(1, 0, 0).is_empty());
        let h = hits(&steps("x_.x").expect("s"), 0.25, 2.0);
        assert_eq!(
            h,
            vec![
                (0.0, 0.5, false),
                (0.75, 0.25, false),
                (1.0, 0.5, false),
                (1.75, 0.25, false)
            ]
        );
    }

    #[test]
    fn generators_produce_notes_in_time() {
        let key = Scale::parse("A minor").expect("key");
        let chords = progression("i VI III VII", Some(&key)).expect("prog");
        let pad = chord_notes(&chords, 4.0, 3, Voicing::Close, None, 90);
        assert_eq!(pad.len(), 12);
        assert!(
            pad.iter()
                .all(|n| n.start_beats < 16.0 && n.length_beats == 4.0)
        );
        let stabs = steps("x..x..x.").expect("rhythm");
        let pump = chord_notes(&chords, 4.0, 3, Voicing::Close, Some((&stabs, 0.5)), 90);
        assert_eq!(pump.len(), 4 * 3 * 3);

        let bass = bassline(&chords, 4.0, 1, BassStyle::Octave, None, 100);
        assert!(bass.iter().all(|n| n.note < 60));
        assert_eq!(bass[0].note % 12, 9, "starts on A");
        let walk = bassline(&chords, 4.0, 1, BassStyle::Walking, None, 100);
        assert!(!walk.is_empty());

        let arp = arpeggio(&chords, 4.0, 3, ArpMode::UpDown, 0.25, 2, 0.8, 90, 7);
        assert_eq!(arp.len(), 64);
        assert!(arp.windows(2).all(|w| w[0].start_beats <= w[1].start_beats));
        let r1 = arpeggio(&chords, 4.0, 3, ArpMode::Random, 0.25, 1, 0.5, 90, 7);
        let r2 = arpeggio(&chords, 4.0, 3, ArpMode::Random, 0.25, 1, 0.5, 90, 7);
        assert_eq!(r1, r2, "seeded");

        let beat = drums(
            &[
                (36, steps("x...x...x...x...").expect("k")),
                (42, euclid(5, 8, 0)),
            ],
            0.25,
            4.0,
            100,
        );
        assert_eq!(beat.iter().filter(|n| n.note == 36).count(), 4);
        assert_eq!(beat.iter().filter(|n| n.note == 42).count(), 10);
    }

    #[test]
    fn transforms() {
        let notes = vec![
            Note {
                note: 61,
                start_beats: 0.05,
                length_beats: 1.0,
                velocity: 100,
            },
            Note {
                note: 64,
                start_beats: 1.0,
                length_beats: 1.0,
                velocity: 40,
            },
        ];
        let t = |ops: &[Transform]| transform(&notes, ops, 4.0).expect("transform");
        assert_eq!(t(&[Transform::Transpose { semitones: 12 }])[0].note, 73);
        assert_eq!(
            t(&[Transform::ToScale {
                key: "C major".into()
            }])[0]
                .note,
            60
        );
        assert_eq!(
            t(&[Transform::Quantize {
                grid: 0.25,
                strength: None
            }])[0]
                .start_beats,
            0.0
        );
        let rev = t(&[Transform::Reverse]);
        assert!((rev[0].start_beats - 2.0).abs() < 1e-9, "{rev:?}");
        let half = t(&[Transform::Stretch { factor: 2.0 }]);
        assert_eq!(half[1].start_beats, 2.0);
        let soft = t(&[Transform::Velocity {
            scale: Some(0.5),
            offset: Some(-60),
        }]);
        assert_eq!(soft[1].velocity, 1, "clamped");
        let thin = t(&[Transform::Thin {
            every: None,
            min_velocity: Some(50),
        }]);
        assert_eq!(thin.len(), 1);
        let h1 = t(&[Transform::Humanize {
            timing: Some(0.05),
            velocity: Some(10),
            seed: Some(3),
        }]);
        let h2 = t(&[Transform::Humanize {
            timing: Some(0.05),
            velocity: Some(10),
            seed: Some(3),
        }]);
        assert_eq!(h1, h2);
        assert!(h1.iter().all(|n| n.start_beats >= 0.0));
        // stretched past the clip: dropped or shortened
        let long = t(&[Transform::Stretch { factor: 4.0 }]);
        assert!(
            long.iter()
                .all(|n| n.start_beats + n.length_beats <= 4.0 + 1e-9)
        );
        assert!(transform(&notes, &[Transform::Stretch { factor: 0.0 }], 4.0).is_err());
        assert!(transform(&notes, &[Transform::ToScale { key: "nope".into() }], 4.0).is_err());
    }
}
