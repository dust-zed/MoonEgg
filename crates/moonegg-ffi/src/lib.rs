#[uniffi::export]
pub fn core_version() -> String {
    moonegg_core::version().to_owned()
}

uniffi::setup_scaffolding!();
