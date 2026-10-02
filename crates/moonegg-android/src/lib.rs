#[cfg(target_os = "android")]
mod aaudio;

#[cfg(target_os = "android")]
mod audio_output;

#[cfg(target_os = "android")]
mod file_source;

#[cfg(target_os = "android")]
mod media_extractor;

#[cfg(target_os = "android")]
mod media_format;

#[cfg(target_os = "android")]
mod track_probe;

#[cfg(target_os = "android")]
mod android_demuxer;

#[cfg(target_os = "android")]
mod decoder_format;

#[cfg(target_os = "android")]
mod media_codec;

#[cfg(target_os = "android")]
mod pcm_output;

#[cfg(target_os = "android")]
mod android_decoder;

#[cfg(target_os = "android")]
mod audio_backend_factory;

#[cfg(target_os = "android")]
mod video_codec_session;

#[cfg(target_os = "android")]
mod video_buffer;

#[cfg(target_os = "android")]
pub use audio_output::{AndroidAudioOutput, AndroidAudioOutputFactory};

#[cfg(target_os = "android")]
pub use file_source::duplicate_file_descriptor;
#[cfg(target_os = "android")]
use moonegg_core::PlayerEngine;

#[cfg(target_os = "android")]
pub fn new_aac_player(
    source: moonegg_core::FileSource,
) -> std::io::Result<moonegg_core::PlayerEngine> {
    let backend_factory = audio_backend_factory::AndroidAudioBackendFactory::new(source);
    let engine = PlayerEngine::new_with_backend(backend_factory, AndroidAudioOutputFactory)?;
    Ok(engine)
}
