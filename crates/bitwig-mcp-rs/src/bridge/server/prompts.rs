//! Prompts: guided workflows the client can offer.

use super::*;

/// Prompt: song sketch.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SketchArgs {
    /// Genre or mood, e.g. "deep house", "lo-fi hip hop".
    pub genre: String,
    /// Tempo in BPM.
    pub tempo: Option<String>,
    /// Key, e.g. "A minor".
    pub key: Option<String>,
}

/// Prompt: hardware setup.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct HardwarePromptArgs {
    /// Synth model, e.g. "Korg Minilogue XD".
    pub synth: String,
}

/// Prompt: compose.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ComposeArgs {
    /// Idea, genre or mood, e.g. "dark techno at 130", "dreamy synthwave".
    pub idea: String,
    /// Hardware synths to use, e.g. "Model D for bass, 2600 for leads".
    pub hardware: Option<String>,
}

#[prompt_router(vis = "pub(crate)")]
impl BridgeServer {
    #[prompt(
        description = "Compose a track step by step with the composing tools, asking the user at each stage. Keeps the song plan in the project."
    )]
    async fn compose_track(&self, Parameters(a): Parameters<ComposeArgs>) -> Vec<PromptMessage> {
        let hardware = a
            .hardware
            .map(|h| format!("Hardware to use: {h}. Check list_instruments for their channels and what they can do; play them through HW Instrument tracks.\n"))
            .unwrap_or_default();
        vec![PromptMessage::new_text(
            Role::User,
            format!(
                "Compose with me in Bitwig: {idea}.\n{hardware}\
                 Work in stages and after each one tell me what you did and ask: keep, vary or redo?\n\
                 0. get_song_plan: if a plan exists, continue it instead of starting over. get_project to see what exists; never delete my material.\n\
                 1. Concept: pick key, tempo, time signature and 3-5 sections (e.g. intro 8 bars, build 8, drop 16, break 8, outro 8). set_transport tempo, then set_song_plan with all of it.\n\
                 2. Drums: create_track instrument \"Drums\" with device \"Drum Machine\"; write_drums per section (vary hats and fills; keep the kick of the genre).\n\
                 3. Bass: write_bassline following the chord progression of each section.\n\
                 4. Harmony: write_chords (voicing close for pads, rhythm for stabs) on a pad or keys track.\n\
                 5. Lead / arp: write_arp or a melody with create_clip notes in the key; leave space for the vocal or hook.\n\
                 6. Sections: create_scene per section and put each part's clip in the section's slot (slot = scene name). Use transform_clip for variations (transpose, thin, humanize) instead of rewriting.\n\
                 7. Mix: set_track volumes in dB (drums and bass loudest, pads -10 dB or lower), pan supporting parts, sends to an effect track with reverb.\n\
                 8. Update set_song_plan with what was built, then launch the first scene.\n\
                 Keep every part in the key ({{key}} from the plan); use Roman numerals so changes of key stay easy.",
                idea = a.idea
            ),
        )]
    }

    #[prompt(
        description = "Sketch a song: tracks with instruments, scenes for the sections, and clips with notes."
    )]
    async fn sketch_song(&self, Parameters(a): Parameters<SketchArgs>) -> Vec<PromptMessage> {
        let tempo = a.tempo.unwrap_or_else(|| "a fitting tempo".into());
        let key = a.key.unwrap_or_else(|| "a fitting key".into());
        vec![PromptMessage::new_text(
            Role::User,
            format!(
                "Sketch a {genre} track in Bitwig at {tempo} in {key}.\n\
                 1. get_project to see what exists; don't delete anything.\n\
                 2. set_transport tempo.\n\
                 3. create_track for drums, bass, chords and a lead, each with a fitting Bitwig \
                 device (Drum Machine, Polymer, Phase-4, ...), a name and a color.\n\
                 4. create_scene Intro, Verse, Chorus, Outro.\n\
                 5. create_clip per track and scene with idiomatic {genre} notes (4 or 8 bars).\n\
                 6. Launch the Verse scene, then summarize what you built.",
                genre = a.genre
            ),
        )]
    }

    #[prompt(description = "Check the mix: levels, panning, mutes and solos, with concrete fixes.")]
    async fn mix_check(&self) -> Vec<PromptMessage> {
        vec![PromptMessage::new_text(
            Role::User,
            "Read get_project and review the mix: tracks far above or below the others, \
             everything panned center, forgotten solos/mutes, sends without effect tracks, a \
             master above 0 dB. Propose concrete changes (in dB) and apply them with set_track \
             only after I agree.",
        )]
    }

    #[prompt(
        description = "Set up an external hardware synth: HW Instrument track, test notes, and an instrument profile."
    )]
    async fn setup_hardware_synth(
        &self,
        Parameters(a): Parameters<HardwarePromptArgs>,
    ) -> Vec<PromptMessage> {
        vec![PromptMessage::new_text(
            Role::User,
            format!(
                "Help me use my {synth} from Bitwig.\n\
                 1. list_instruments: if a profile for the {synth} exists, use its channel, \
                 notes and parameters; otherwise write one (TOML, from its MIDI implementation \
                 chart) for the instruments folder that list_instruments reports, and check it loads.\n\
                 2. create_track kind instrument named \"{synth}\" with device \"HW Instrument\".\n\
                 3. Tell me to pick the MIDI output port, the MIDI channel from the profile and \
                 the audio input in that device, and to set the synth to the same channel.\n\
                 4. play_notes a chord (a single note for mono synths) to test.\n\
                 5. Try a hardware_set parameter from the profile, if it has any, and explain \
                 which knobs stay manual.",
                synth = a.synth
            ),
        )]
    }
}
