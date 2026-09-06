// 播放核心永远保持 Safe Rust
#![forbid(unsafe_code)]

mod engine;
mod error;
mod metrics;

mod media;
mod pipeline;
mod player;
mod ports;
mod runtime;
mod timing;

#[must_use]
pub const fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_matches_package_version() {
        assert_eq!(version(), env!("CARGO_PKG_VERSION"));
    }
}
