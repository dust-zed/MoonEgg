use std::{
    path::PathBuf,
    sync::mpsc::RecvTimeoutError,
    time::{Duration, Instant},
};

use crate::{
    engine::PlayerEngine,
    player::{PlayerCommand, PlayerEvent, PlayerState},
    runtime::{ControlLoopExit, EffectLoopExit, ThreadTermination, WavPlaybackFactory},
};

pub fn run_wav_demo(path: PathBuf) -> Result<(), String> {
    let factory = WavPlaybackFactory::new(path);

    let engine =
        PlayerEngine::new_audio(factory).map_err(|error| format!("创建引擎失败：{error}"))?;

    let result = drive_demo(&engine);

    let report = engine.shutdown();
    println!("关闭结果： {report:?}");

    let shutdown_ok = report.release_sent
        && matches!(
            report.control,
            ThreadTermination::Exited(ControlLoopExit::Released)
        )
        && matches!(
            report.effect,
            ThreadTermination::Exited(EffectLoopExit::Released)
        );

    result?;

    if !shutdown_ok {
        return Err("引擎未正常关闭， 请查看关闭结果".to_owned());
    }

    Ok(())
}

fn drive_demo(engine: &PlayerEngine) -> Result<(), String> {
    engine
        .send_command(PlayerCommand::Prepare)
        .map_err(|error| format!("发送 prepare 失败：{error}"))?;
    //最多等待 10 秒进入 Playing
    let mut deadline = Instant::now() + Duration::from_secs(10);
    let mut play_sent = false;
    let mut playing = false;

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());

        if remaining.is_zero() {
            return if playing {
                return Ok(());
            } else {
                Err("等待进入 Playing 超时".to_owned())
            };
        }

        let event = match engine.recv_event_timeout(remaining) {
            Ok(event) => event,

            Err(RecvTimeoutError::Timeout) => continue,

            Err(RecvTimeoutError::Disconnected) => return Err("事件通道已断开".to_owned()),
        };

        println!("{event:?}");

        match event {
            PlayerEvent::StateChanged {
                current: PlayerState::Ready,
                ..
            } if !play_sent => {
                engine
                    .send_command(PlayerCommand::Play)
                    .map_err(|error| format!("发送 Play 失败：{error:?}"))?;

                play_sent = true;
            }

            PlayerEvent::StateChanged {
                current: PlayerState::Playing,
                ..
            } if !playing => {
                playing = true;

                deadline = Instant::now() + Duration::from_secs(5);
            }

            PlayerEvent::PlaybackFailed { error } => {
                return Err(format!("播放失败： {error:?}"));
            }

            PlayerEvent::CommandRejected { command, state } => {
                return Err(format!("命令被拒绝: {command:?}, 当前状态： {state:?}"));
            }

            _ => {}
        }
    }
}
