#[cfg(target_os = "android")]
mod aaudio;

#[cfg(target_os = "android")]
mod audio_output;

#[cfg(target_os = "android")]
mod file_source;

#[cfg(target_os = "android")]
pub use audio_output::{AndroidAudioOutput, AndroidAudioOutputFactory};
