mod event;
mod player;
#[cfg(target_os = "android")]
mod surface_registry;

#[cfg(target_os = "android")]
mod surface_jni;

pub use player::{NativePlayer, PlayerBridgeError};

#[uniffi::export]
pub fn core_version() -> String {
    moonegg_core::version().to_owned()
}

uniffi::setup_scaffolding!();
