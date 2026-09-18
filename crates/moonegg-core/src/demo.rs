use std::{
    path::PathBuf,
    sync::mpsc::RecvTimeoutError,
    time::{Duration, Instant},
};

use crate::{
    engine::PlayerEngine,
    media::MediaTime,
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
    let steps = [
        (PlayerCommand::Prepare, PlayerState::Ready, 0),
        // 第一遍：等到自然结束。
        (PlayerCommand::Play, PlayerState::Ended, 0),
        // 定位回开头，保持暂停。
        (
            PlayerCommand::Seek(MediaTime::from_nanoseconds(0)),
            PlayerState::Paused,
            0,
        ),
        // 第二遍：再次等到自然结束。
        (PlayerCommand::Play, PlayerState::Ended, 0),
        // 释放当前播放会话，回到 Idle。
        (PlayerCommand::Stop, PlayerState::Idle, 0),
    ];

    for (command, expected_state, observe_secs) in steps {
        println!("发送命令： {command:?}");

        engine
            .send_command(command)
            .map_err(|error| format!("发送 {command:?} 失败：{error:?}"))?;

        //每一步先等待目标状态，最多等待 10 秒
        let mut state_reached = false;
        let mut deadline = Instant::now() + Duration::from_secs(10);

        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());

            if remaining.is_zero() {
                if state_reached {
                    break;
                }
                return Err(format!("等待进入 {expected_state:?} 超时"));
            }

            let event = match engine.recv_event_timeout(remaining) {
                Ok(event) => event,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => {
                    return Err("事件通道已断开".to_owned());
                }
            };

            println!("{event:?}");

            match event {
                PlayerEvent::StateChanged { current, .. } => {
                    if !state_reached && current == expected_state {
                        state_reached = true;

                        // Ready 不需要停留，马上进入播放步骤。
                        if observe_secs == 0 {
                            break;
                        }

                        // 从等待状态切换为观察进度
                        deadline = Instant::now() + Duration::from_secs(observe_secs);
                    }
                }
                PlayerEvent::PlaybackFailed { error } => {
                    return Err(format!("播放失败：{error:?}"));
                }

                PlayerEvent::CommandRejected { command, state } => {
                    return Err(format!("命令被拒绝：{command:?}, 当前状态： {state:?}"));
                }

                _ => {}
            }
        }
    }
    Ok(())
}
