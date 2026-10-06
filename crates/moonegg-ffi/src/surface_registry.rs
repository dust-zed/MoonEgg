use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

use ndk::native_window::NativeWindow;

#[derive(Debug, thiserror::Error)]
pub(crate) enum SurfaceRegistryError {
    #[error("视频窗口句柄已耗尽")]
    HandleExhausted,
    #[error("视频窗口不存在或已被取走： handle={handle}")]
    UnknownHandle { handle: i64 },
    #[error("视频窗口句柄表的锁已损坏")]
    LockPoisoned,
}

/// 暂存尚未交给播放器的窗口
pub(crate) struct SurfaceRegistry {
    /// 下一个编号，从 1 开始，递增且不复用。
    next_handle: i64,

    /// 持有窗口所有权，取走和从表中删除
    windows: HashMap<i64, NativeWindow>,
}

impl SurfaceRegistry {
    /// 创建空表
    pub(crate) fn new() -> Self {
        // 编号从 1 开始；0 留给后续 JNI 表示失败
        let next_handle = 1;

        // 初始化空窗口表。
        let windows: HashMap<i64, NativeWindow> = HashMap::new();

        Self {
            next_handle,
            windows,
        }
    }

    /// 保存窗口，返回一次性交接句柄
    pub(crate) fn insert(&mut self, window: NativeWindow) -> Result<i64, SurfaceRegistryError> {
        let handle = self.next_handle;
        let next_handle = handle
            .checked_add(1)
            .ok_or(SurfaceRegistryError::HandleExhausted)?;
        self.windows.entry(handle).or_insert(window);
        self.next_handle = next_handle;
        Ok(handle)
    }

    /// 取走窗口；同一句柄只能成功使用一次
    pub(crate) fn take(&mut self, handle: i64) -> Result<NativeWindow, SurfaceRegistryError> {
        let maybe_window = self.windows.remove(&handle);
        let window = maybe_window.ok_or(SurfaceRegistryError::UnknownHandle { handle })?;
        Ok(window)
    }

    /// 取消交接窗口并释放窗口；重复取消允许成功返回 false。
    pub(crate) fn discard(&mut self, handle: i64) -> bool {
        // 从列表移除窗口
        let maybe_window = self.windows.remove(&handle);
        let removed = maybe_window.is_some();
        // 释放窗口引用
        std::mem::drop(maybe_window);
        removed
    }
}
/// 整个进程共用一张表
static SURFACE_REGISTRY: OnceLock<Mutex<SurfaceRegistry>> = OnceLock::new();

/// 获取唯一的窗口表。
fn registry() -> &'static Mutex<SurfaceRegistry> {
    // get_or_init: 初始化
    let registry = SURFACE_REGISTRY.get_or_init(|| Mutex::new(SurfaceRegistry::new()));
    registry
}

/// 保存窗口，供 JNI 注册入口调用。
pub(crate) fn registry_window(window: NativeWindow) -> Result<i64, SurfaceRegistryError> {
    let mut guard = registry()
        .lock()
        .map_err(|_| SurfaceRegistryError::LockPoisoned)?;
    let handle = guard.insert(window)?;
    Ok(handle)
}

/// 领取窗口， 供播放器构造方法使用
pub(crate) fn take_window(handle: i64) -> Result<NativeWindow, SurfaceRegistryError> {
    // 获取锁
    let mut guard = registry()
        .lock()
        .map_err(|_| SurfaceRegistryError::LockPoisoned)?;
    let window = guard.take(handle)?;
    Ok(window)
}

/// 取消尚未完成的窗口交接
pub(crate) fn discard_window(handle: i64) -> Result<bool, SurfaceRegistryError> {
    // 获取锁
    let mut guard = registry()
        .lock()
        .map_err(|_| SurfaceRegistryError::LockPoisoned)?;
    Ok(guard.discard(handle))
}
