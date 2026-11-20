//! The sounds of a notification that rings (#1002, `docs/design.md`, §11): Flottille's own, two
//! short ones committed under `assets/sounds/`, played by the daemon through the sound server
//! (PulseAudio's protocol, which PipeWire answers too) so that they sound when the window is
//! closed. Never by running `paplay` or `aplay`. Without a sound server the daemon says it once
//! and plays nothing.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use flottille_core::paths::Paths;

/// The file of the daemon's configuration folder that chooses the sink: `backend = "fake"` and
/// `file = "<path>"` write each sound's name to that file instead of playing it. A test build's
/// alone (`cargo test` builds without optimisation, a release with): a user's file that asks for
/// it is not read, and no sound is lost to it.
const CHOICE: &str = "audio.toml";

/// Which sound a notification makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sound {
    /// What needs the user: a request, a question.
    NeedsYou,
    /// The rest.
    Tell,
}

impl Sound {
    pub fn name(self) -> &'static str {
        match self {
            Sound::NeedsYou => "needs-you",
            Sound::Tell => "tell",
        }
    }

    fn wav(self) -> &'static [u8] {
        match self {
            Sound::NeedsYou => include_bytes!("../../../assets/sounds/needs-you.wav"),
            Sound::Tell => include_bytes!("../../../assets/sounds/tell.wav"),
        }
    }
}

/// A WAV file's samples: mono, 16 bits, little endian.
#[derive(Debug, PartialEq, Eq)]
pub struct Pcm {
    pub rate: u32,
    pub bytes: Vec<u8>,
}

impl Pcm {
    /// The samples of a canonical WAV file: its `fmt ` chunk says mono, 16 bits, PCM, and its
    /// `data` chunk holds the samples.
    pub fn of(wav: &[u8]) -> Option<Pcm> {
        if wav.get(..4)? != b"RIFF" || wav.get(8..12)? != b"WAVE" {
            return None;
        }
        let (mut at, mut rate, mut data) = (12, None, None);
        while at + 8 <= wav.len() {
            let id = &wav[at..at + 4];
            let size = u32::from_le_bytes(wav[at + 4..at + 8].try_into().ok()?) as usize;
            let body = wav.get(at + 8..at + 8 + size)?;
            match id {
                b"fmt " => {
                    let format = u16::from_le_bytes(body.get(0..2)?.try_into().ok()?);
                    let channels = u16::from_le_bytes(body.get(2..4)?.try_into().ok()?);
                    let bits = u16::from_le_bytes(body.get(14..16)?.try_into().ok()?);
                    if (format, channels, bits) != (1, 1, 16) {
                        return None;
                    }
                    rate = Some(u32::from_le_bytes(body.get(4..8)?.try_into().ok()?));
                }
                b"data" => data = Some(body.to_vec()),
                _ => {}
            }
            // Chunks are padded to an even size.
            at += 8 + size + (size & 1);
        }
        Some(Pcm {
            rate: rate?,
            bytes: data?,
        })
    }

    /// The same samples at `volume` (1 is as made), never past full scale.
    pub fn at(&self, volume: f32) -> Vec<u8> {
        self.bytes
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|pair| {
                let sample = i16::from_le_bytes(*pair) as f32 * volume;
                (sample.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16).to_le_bytes()
            })
            .collect()
    }
}

enum Backend {
    /// The sound server of the user's session.
    System,
    /// A test's: each sound's name written to the file, one a line.
    Fake(PathBuf),
}

/// Plays the sounds.
pub struct Sounds {
    backend: Backend,
    /// Why there is no sound, once said.
    said: AtomicBool,
    /// Which sounds are playing now: a second request in the same instant waits for the first's
    /// end rather than mixing into it.
    playing: tokio::sync::Mutex<()>,
    /// What was last said of the device, for Settings.
    why: Mutex<Option<String>>,
}

impl Sounds {
    pub fn open(paths: &Paths) -> Sounds {
        let backend = match (cfg!(debug_assertions), choice(paths)) {
            (true, Some(file)) => Backend::Fake(file),
            _ => Backend::System,
        };
        Sounds {
            backend,
            said: AtomicBool::new(false),
            playing: tokio::sync::Mutex::new(()),
            why: Mutex::new(None),
        }
    }

    /// Why no sound is played, when it is the case.
    pub fn why_not(&self) -> Option<String> {
        self.why.lock().unwrap().clone()
    }

    /// Plays `sound` at `volume`, and says once, in the daemon's log, why when it cannot.
    pub async fn play(&self, sound: Sound, volume: f32) {
        let _one_at_a_time = self.playing.lock().await;
        let played = match &self.backend {
            Backend::Fake(file) => record(file, sound),
            Backend::System => play_on_the_system(sound, volume).await,
        };
        match played {
            Ok(()) => {
                self.said.store(false, Ordering::Relaxed);
                *self.why.lock().unwrap() = None;
            }
            Err(why) => {
                *self.why.lock().unwrap() = Some(why.clone());
                if !self.said.swap(true, Ordering::Relaxed) {
                    eprintln!(
                        "flottille daemon: {why}: notifications make no sound until a sound \
                         server answers"
                    );
                }
            }
        }
    }
}

fn choice(paths: &Paths) -> Option<PathBuf> {
    let document = std::fs::read_to_string(paths.config.join(CHOICE))
        .ok()?
        .parse::<toml_edit::DocumentMut>()
        .ok()?;
    match document.get("backend")?.as_str()? {
        "fake" => Some(PathBuf::from(document.get("file")?.as_str()?)),
        _ => None,
    }
}

fn record(file: &std::path::Path, sound: Sound) -> Result<(), String> {
    use std::io::Write as _;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
        .and_then(|mut file| writeln!(file, "{}", sound.name()))
        .map_err(|error| format!("the fake sink cannot be written: {error}"))
}

/// The sound server's stream: the samples sent as it asks for them, then drained.
async fn play_on_the_system(sound: Sound, volume: f32) -> Result<(), String> {
    use pulseaudio::{AsPlaybackSource as _, Client, protocol};

    let pcm = Pcm::of(sound.wav()).ok_or("a sound of Flottille's is not a WAV file")?;
    let samples = pcm.at(volume);
    let client = tokio::task::spawn_blocking(|| Client::from_env(c"Flottille"))
        .await
        .map_err(|error| format!("no audio device: {error}"))?
        .map_err(|error| format!("no audio device: no sound server answers ({error})"))?;
    let params = protocol::PlaybackStreamParams {
        sample_spec: protocol::SampleSpec {
            format: protocol::SampleFormat::S16Le,
            channels: 1,
            sample_rate: pcm.rate,
        },
        channel_map: protocol::ChannelMap::mono(),
        cvolume: Some(protocol::ChannelVolume::norm(1)),
        sink_name: Some(protocol::DEFAULT_SINK.to_owned()),
        ..Default::default()
    };
    let mut sent = 0;
    let source = move |buffer: &mut [u8]| -> usize {
        let count = buffer.len().min(samples.len() - sent) & !1;
        buffer[..count].copy_from_slice(&samples[sent..sent + count]);
        sent += count;
        count
    };
    let stream = client
        .create_playback_stream(params, source.as_playback_source())
        .await
        .map_err(|error| format!("no audio device: the stream cannot start ({error})"))?;
    stream
        .play_all()
        .await
        .map_err(|error| format!("no audio device: the sound cannot be played ({error})"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sound_is_short_mono_and_quiet() {
        for sound in [Sound::NeedsYou, Sound::Tell] {
            let pcm = Pcm::of(sound.wav()).expect("a WAV file");
            let samples = pcm.bytes.len() / 2;
            let seconds = samples as f32 / pcm.rate as f32;
            assert!(
                seconds > 0.1 && seconds < 0.6,
                "{sound:?} lasts {seconds} s"
            );
            let peak = pcm
                .bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| i16::from_le_bytes(*pair).unsigned_abs())
                .max()
                .unwrap();
            assert!(peak > 3_000 && peak < 16_384, "{sound:?} peaks at {peak}");
        }
    }

    #[test]
    fn a_volume_scales_the_samples_and_never_passes_full_scale() {
        let pcm = Pcm {
            rate: 22_050,
            bytes: [1_000i16, -1_000, 30_000]
                .iter()
                .flat_map(|s| s.to_le_bytes())
                .collect(),
        };
        let half: Vec<i16> = pcm
            .at(0.5)
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| i16::from_le_bytes(*p))
            .collect();
        assert_eq!(half, [500, -500, 15_000]);
        let loud: Vec<i16> = pcm
            .at(4.0)
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| i16::from_le_bytes(*p))
            .collect();
        assert_eq!(loud, [4_000, -4_000, i16::MAX]);
    }

    #[test]
    fn what_is_not_a_mono_16_bit_wav_is_refused() {
        assert_eq!(Pcm::of(b"not a wav file at all"), None);
        let mut stereo = Sound::Tell.wav().to_vec();
        stereo[22] = 2;
        assert_eq!(Pcm::of(&stereo), None);
    }
}
