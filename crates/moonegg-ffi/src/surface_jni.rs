use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::surface_registry::{SurfaceRegistryError, discard_window, registry_window};
use jni::{JNIEnv, objects::JObject, sys::jlong};
use moonegg_android::{VideoSurfaceError, acquire_native_window};

#[derive(Debug, thiserror::Error)]
enum SurfaceJniError {
    #[error("Surface 转换失败：{0}")]
    Surface(#[from] VideoSurfaceError),
    #[error("窗口句柄操作失败：{0}")]
    Registry(#[from] SurfaceRegistryError),
    #[error("JNI 调用失败： {0}")]
    Jni(#[from] jni::errors::Error),
    #[error("参数不是 android.view.Surface")]
    WrongSurfaceType,
    #[error("视频窗口桥接发生Rust panic")]
    RustPanic,
}

/// 验证 Surface，获取窗口并登记
fn register_surface(env: &mut JNIEnv<'_>, surface: &JObject<'_>) -> Result<i64, SurfaceJniError> {
    if surface.is_null() {
        return Err(SurfaceJniError::Surface(VideoSurfaceError::NullSurface));
    }
    let is_surface = env.is_instance_of(surface, "android/view/Surface")?;
    if !is_surface {
        return Err(SurfaceJniError::WrongSurfaceType);
    }
    // SAFETY：当前 JNI 环境有效，Surface 类型已验证
    let window_result =
        unsafe { acquire_native_window(env.get_native_interface(), surface.as_raw()) };
    let expection_pending = env.exception_check()?;
    if expection_pending {
        return Err(SurfaceJniError::Jni(jni::errors::Error::JavaException));
    }
    let window = window_result?;
    let handle = registry_window(window)?;
    Ok(handle)
}

/// 报告错误，保留已有 Java 异常
fn throw_bridge_error(
    env: &mut JNIEnv<'_>,
    error: &SurfaceJniError,
) -> Result<(), jni::errors::Error> {
    let exception_pending = env.exception_check()?;

    if exception_pending {
        return Ok(());
    }
    let message = error.to_string();
    env.throw_new("java/lang/IllegalStateException", message)
}
/// 统一处理 JNI 返回值、业务错误和 rust panic
fn run_jni<'local, T: Default>(
    env: &mut JNIEnv<'local>,
    operation: impl FnOnce(&mut JNIEnv<'local>) -> Result<T, SurfaceJniError>,
) -> T {
    let unwind_result = catch_unwind(AssertUnwindSafe(|| {
        let exception_pending = env.exception_check()?;
        if exception_pending {
            return Err(SurfaceJniError::Jni(jni::errors::Error::JavaException));
        }
        operation(env)
    }));
    let operation_result = match unwind_result {
        // 没有 panic，保留工作的成功或失败结果
        Ok(result) => result,
        // 发生 panic，转换成我们定义的错误
        Err(_payload) => Err(SurfaceJniError::RustPanic),
    };

    match operation_result {
        Ok(value) => value,
        Err(error) => {
            let _ = throw_bridge_error(env, &error);
            T::default()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_dustzed_moonegg_player_NativeSurfaceBridge_nativeRegisterSurface(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    surface: JObject<'_>,
) -> jlong {
    // run_jni(&mut env, |env| register_surface(env, &surface))。
    // 成功返回正数句柄，错误时抛 Java 异常并返回 0。
    let handle: jlong = run_jni(&mut env, |env| register_surface(env, &surface));

    handle
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_dustzed_moonegg_player_NativeSurfaceBridge_nativeDiscardSurface(
    mut env: JNIEnv<'_>,
    _receiver: JObject<'_>,
    handle: jlong,
) {
    // run_jni::<()>。
    // 闭包调用 discard_window(handle)，使用 ?。
    // 已取走或已取消时仍返回 Ok(())。
    let result = run_jni::<()>(&mut env, |_env| {
        let _removed = discard_window(handle)?;
        Ok(())
    });

    result
}
