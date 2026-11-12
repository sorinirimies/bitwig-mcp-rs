//! A short guide to Bitwig's own devices (those in the Essentials edition and up): what each
//! is for and how to get typical sounds from it. Attached to `list_devices` results so the
//! model can pick and shape instruments without trial and error. Pure data.

use serde::Serialize;

/// One device.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct Guide {
    /// Name as in Bitwig's browser.
    pub name: &'static str,
    /// instrument, audio_fx, note_fx, drum or utility.
    pub kind: &'static str,
    /// What it is for.
    pub role: &'static str,
    /// Recipes and tips.
    pub tips: &'static str,
}

/// The guide.
pub const GUIDE: &[Guide] = &[
    Guide {
        name: "Polymer",
        kind: "instrument",
        role: "hybrid synth (oscillator, filter, envelopes): basses, leads, pads, plucks",
        tips: "bass: saw or pulse, filter low (25-40%), short amp decay, resonance up for acid; pad: two detuned saws, slow attack (0.5-1 s), long release, filter half open; pluck: decay 150-300 ms, sustain 0, filter envelope amount up",
    },
    Guide {
        name: "Organ",
        kind: "instrument",
        role: "drawbar-style organ: chords, house stabs, gospel",
        tips: "house stab: short notes with write_chords rhythm, a touch of Reverb; pads: hold chords, add Phaser or Flanger slowly",
    },
    Guide {
        name: "Sampler",
        kind: "instrument",
        role: "plays audio files across the keyboard: one-shots, keys, vocal chops",
        tips: "load a sample by file path with insert_device file; low notes on a sample are slow and dark",
    },
    Guide {
        name: "Drum Machine",
        kind: "drum",
        role: "drum rack: one instrument per pad (key), General MIDI layout (C1 = 36 kick)",
        tips: "empty by default: build_drum_kit puts v8 drums on the pads; then write_drums uses the same key names",
    },
    Guide {
        name: "v8 Kick",
        kind: "drum",
        role: "808-style kick",
        tips: "long decay for trap and techno subs, short for house; tune to the key root",
    },
    Guide {
        name: "v8 Snare",
        kind: "drum",
        role: "808-style snare",
        tips: "on beats 2 and 4; layer with v8 Clap for weight",
    },
    Guide {
        name: "v8 Clap",
        kind: "drum",
        role: "808-style clap",
        tips: "backbeat; a little Reverb on a send",
    },
    Guide {
        name: "v8 Hat",
        kind: "drum",
        role: "808-style hi-hat (closed and open via decay)",
        tips: "16ths with accents; open hat on the off-beats",
    },
    Guide {
        name: "v8 Cymbal",
        kind: "drum",
        role: "808-style cymbal",
        tips: "first beat of a section, or quiet 8ths",
    },
    Guide {
        name: "v8 Tom",
        kind: "drum",
        role: "808-style tom",
        tips: "fills at the end of 4 or 8 bars",
    },
    Guide {
        name: "v8 Rimshot",
        kind: "drum",
        role: "808-style rimshot",
        tips: "Euclidean 3 or 5 in 8 for groove",
    },
    Guide {
        name: "v8 Claves",
        kind: "drum",
        role: "808-style claves",
        tips: "percussion accents",
    },
    Guide {
        name: "v8 Cowbell",
        kind: "drum",
        role: "808-style cowbell",
        tips: "sparing, syncopated",
    },
    Guide {
        name: "v8 Maracas",
        kind: "drum",
        role: "808-style maracas",
        tips: "steady 16ths, low velocity",
    },
    Guide {
        name: "HW Instrument",
        kind: "instrument",
        role: "plays an external synth over MIDI and returns its audio",
        tips: "pick the MIDI port and channel of the synth's profile, and the audio inputs it is wired to",
    },
    Guide {
        name: "EQ-5",
        kind: "audio_fx",
        role: "5-band parametric EQ",
        tips: "high-pass non-bass parts at 100-200 Hz; cut mud at 250-400 Hz; boost air at 10 kHz gently",
    },
    Guide {
        name: "EQ-2",
        kind: "audio_fx",
        role: "simple 2-band EQ",
        tips: "quick tone shaping",
    },
    Guide {
        name: "EQ-DJ",
        kind: "audio_fx",
        role: "DJ-style 3-band kill EQ",
        tips: "bass swaps and breakdowns",
    },
    Guide {
        name: "Compressor",
        kind: "audio_fx",
        role: "compressor",
        tips: "drums: ratio 4:1, fast attack; bus glue: 2:1, slow attack, 1-3 dB gain reduction",
    },
    Guide {
        name: "Dynamics",
        kind: "audio_fx",
        role: "up/down compression and expansion",
        tips: "tame peaks or bring up quiet detail",
    },
    Guide {
        name: "Peak Limiter",
        kind: "audio_fx",
        role: "limiter",
        tips: "last on the master; ceiling -1 dB; raise input for loudness",
    },
    Guide {
        name: "Gate",
        kind: "audio_fx",
        role: "noise gate",
        tips: "tighten drums, trance-gate effects with a short release",
    },
    Guide {
        name: "Delay+",
        kind: "audio_fx",
        role: "tempo-synced delay",
        tips: "1/8 dotted on leads and arps; on a send at -15 dB",
    },
    Guide {
        name: "Reverb",
        kind: "audio_fx",
        role: "algorithmic reverb",
        tips: "on an effect track with sends; pre-delay 20-40 ms keeps clarity",
    },
    Guide {
        name: "Filter",
        kind: "audio_fx",
        role: "multimode filter",
        tips: "sweeps for builds (record_automation)",
    },
    Guide {
        name: "Ladder",
        kind: "audio_fx",
        role: "ladder low-pass filter",
        tips: "warm acid and resonant sweeps",
    },
    Guide {
        name: "Distortion",
        kind: "audio_fx",
        role: "distortion",
        tips: "grit on bass (mix low), drive on drums",
    },
    Guide {
        name: "Bit-8",
        kind: "audio_fx",
        role: "bit crusher and downsampler",
        tips: "lo-fi textures",
    },
    Guide {
        name: "Phaser",
        kind: "audio_fx",
        role: "phaser",
        tips: "movement on pads and keys",
    },
    Guide {
        name: "Flanger",
        kind: "audio_fx",
        role: "flanger",
        tips: "jet sweeps on hats or pads",
    },
    Guide {
        name: "Tool",
        kind: "utility",
        role: "gain, pan, width, phase",
        tips: "gain staging and mono bass",
    },
    Guide {
        name: "Arpeggiator",
        kind: "note_fx",
        role: "live arpeggiator",
        tips: "alternative to write_arp: hold chords, the device arpeggiates",
    },
    Guide {
        name: "Stepwise",
        kind: "note_fx",
        role: "step sequencer note effect",
        tips: "rhythmic gating of held notes",
    },
    Guide {
        name: "Note Transpose",
        kind: "note_fx",
        role: "transposes notes",
        tips: "octave layers",
    },
    Guide {
        name: "Key Filter+",
        kind: "note_fx",
        role: "keeps notes in a scale",
        tips: "safety net for live playing",
    },
    Guide {
        name: "Velocity Curve",
        kind: "note_fx",
        role: "reshapes velocity",
        tips: "even out or exaggerate dynamics",
    },
];

/// The guide entry for a device name (case-insensitive).
#[must_use]
pub fn find(name: &str) -> Option<&'static Guide> {
    GUIDE
        .iter()
        .find(|g| g.name.eq_ignore_ascii_case(name.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_and_findable() {
        let mut names: Vec<String> = GUIDE.iter().map(|g| g.name.to_lowercase()).collect();
        names.sort();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len());
        assert_eq!(find("polymer").map(|g| g.kind), Some("instrument"));
        assert_eq!(find("V8 KICK").map(|g| g.kind), Some("drum"));
        assert!(find("Serum").is_none());
        let kinds = ["instrument", "audio_fx", "note_fx", "drum", "utility"];
        assert!(
            GUIDE
                .iter()
                .all(|g| kinds.contains(&g.kind) && !g.tips.is_empty())
        );
    }
}
