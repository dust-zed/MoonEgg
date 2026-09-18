use std::path::PathBuf;

use crate::scenario::run_wav_demo;

mod scenario;

fn main() {
    let Some(path) = std::env::args_os().nth(1) else {
        eprintln!("用法： wav_demo <WAV 文件路径>");
        std::process::exit(2);
    };

    if let Err(error) = run_wav_demo(PathBuf::from(path)) {
        eprintln!("{error}");
        std::process::exit(1);
    };
}
