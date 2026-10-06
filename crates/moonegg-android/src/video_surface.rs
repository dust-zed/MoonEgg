use jni_sys::{JNIEnv, jobject};
use ndk::native_window::NativeWindow;
#[derive(Debug, thiserror::Error)]
pub enum VideoSurfaceError {
    /// env 是空指针，不能调用 JNI
    #[error("获取视频窗口失败: JNIEnv 为空")]
    NullJniEnv,
    /// 调用方没有提供 Surface
    #[error("获取视频窗口失败： Surface 为空")]
    NullSurface,
    /// 参数通过空指针检查，但底层没有返回窗口
    ///
    /// 转换接口没有提供错误码，因此此处不添加虚构的 status。
    #[error("无法从 Surface 获取 NativeWindow")]
    AcquireFailed,
}

/// 从 Java Surface 获取一个由 Rust 管理的窗口。
/// 返回的窗口可以移入 new_av_player
///
/// # Safety
/// 当前线程有效的 JNI 环境和有效的 Surface 引用
pub unsafe fn acquire_native_window(
    env: *mut JNIEnv,
    surface: jobject,
) -> Result<NativeWindow, VideoSurfaceError> {
    if env.is_null() {
        return Err(VideoSurfaceError::NullJniEnv);
    }
    if surface.is_null() {
        return Err(VideoSurfaceError::NullSurface);
    }
    let maybe_window = unsafe { NativeWindow::from_surface(env, surface) };
    let output_window = maybe_window.ok_or(VideoSurfaceError::AcquireFailed)?;
    Ok(output_window)
}
