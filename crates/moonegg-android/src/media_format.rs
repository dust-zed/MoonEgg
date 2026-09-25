use std::{
    ffi::{CStr, c_char},
    ptr::{NonNull, null},
};

use ndk_sys::{AMediaFormat, AMediaFormat_delete, AMediaFormat_getString};

#[derive(Debug, thiserror::Error)]
pub(crate) enum MediaFormatError {
    #[error("空字符串指针")]
    NullStringPointer,
    #[error("字符串无法严格转换为UTF-8")]
    InvalidUtf8,
}

pub(crate) struct NativeMediaFormat {
    inner: NonNull<AMediaFormat>,
}

impl NativeMediaFormat {
    /// SAFETY: raw_format 必须指向有效的 AMediaFormat
    /// 允许空指针；非空时必须有效且移交唯一所有权
    ///  调用方必须拥有该对象，并将所有权交给此封装。
    /// 移交后，调用方不能再释放它或另建一个拥有者
    pub(crate) unsafe fn from_owned_raw(raw_format: *mut AMediaFormat) -> Option<Self> {
        let inner = NonNull::new(raw_format)?;
        Some(Self { inner })
    }

    pub(crate) fn get_string(&mut self, key: &CStr) -> Result<Option<String>, MediaFormatError> {
        let mut raw_value: *const c_char = null();

        // SAFETY:
        // self 持有有效且未释放的 AMediaFormat
        // key 是有效的、以空字符串结尾的 C 字符串。
        // raw_value 的地址在调用期间有效，可以接收平台写入的指针。
        let found =
            unsafe { AMediaFormat_getString(self.inner.as_ptr(), key.as_ptr(), &mut raw_value) };

        if !found {
            return Ok(None);
        }

        if raw_value.is_null() {
            return Err(MediaFormatError::NullStringPointer);
        }

        // SAFETY:
        // raw_value 来自成功的 NDK 字符串查询，并且已确认非空。
        // 平台保证它指向有效的，以空字符结尾的字符串。
        // self 仍然存活，且期间没有再次调用 getString
        let borrowed_value = unsafe { CStr::from_ptr(raw_value) };

        let text = borrowed_value
            .to_str()
            .map_err(|_| MediaFormatError::InvalidUtf8)?;

        let value = text.to_owned();

        Ok(Some(value))
    }
}

impl Drop for NativeMediaFormat {
    fn drop(&mut self) {
        unsafe { AMediaFormat_delete(self.inner.as_ptr()) };
    }
}
