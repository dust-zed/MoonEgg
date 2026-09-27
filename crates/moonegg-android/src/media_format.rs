use std::{
    ffi::{CStr, c_char},
    os::raw::c_void,
    ptr::{NonNull, null, null_mut},
};

use ndk_sys::{
    AMediaFormat, AMediaFormat_delete, AMediaFormat_getBuffer, AMediaFormat_getInt32,
    AMediaFormat_getInt64, AMediaFormat_getString, AMediaFormat_new, AMediaFormat_setBuffer,
    AMediaFormat_setInt32, AMediaFormat_setString,
};

#[derive(Debug, thiserror::Error)]
pub(crate) enum MediaFormatError {
    #[error("空字符串指针")]
    NullStringPointer,
    #[error("字符串无法严格转换为UTF-8")]
    InvalidUtf8,
    #[error("")]
    NullBufferPointer,
    #[error("")]
    InvalidBufferSize { size: usize },
    #[error("")]
    CreateFailed,
}

pub(crate) struct NativeMediaFormat {
    inner: NonNull<AMediaFormat>,
}

impl NativeMediaFormat {
    /// # SAFETY:
    /// 允许空指针；非空时必须有效且移交唯一所有权
    ///  调用方必须拥有该对象，并将所有权交给此封装。
    /// 移交后，调用方不能再释放它或另建一个拥有者
    pub(crate) unsafe fn from_owned_raw(raw_format: *mut AMediaFormat) -> Option<Self> {
        let inner = NonNull::new(raw_format)?;
        Some(Self { inner })
    }

    pub(crate) fn new() -> Result<Self, MediaFormatError> {
        let raw_format = unsafe { AMediaFormat_new() };
        // SAFETY：非空指针来自本次创建，尚未交给其他拥有者。
        unsafe { Self::from_owned_raw(raw_format) }.ok_or(MediaFormatError::CreateFailed)
    }

    pub(crate) fn set_i32(&mut self, key: &CStr, value: i32) {
        // SAFETY：格式对象有效，key 在调用期间有效，当前独占借用 self。
        unsafe {
            AMediaFormat_setInt32(self.inner.as_ptr(), key.as_ptr(), value);
        }
    }

    pub(crate) fn set_string(&mut self, key: &CStr, value: &CStr) {
        // SAFETY：格式对象有效，key 和 value 都是有效的 C 字符串。
        // 平台复制字符串，调用结束后不再依赖 value 的存储。
        unsafe {
            AMediaFormat_setString(self.inner.as_ptr(), key.as_ptr(), value.as_ptr());
        }
    }

    pub(crate) fn set_buffer(&mut self, key: &CStr, data: &[u8]) {
        let data_len = data.len();
        //    SAFETY：data 在调用期间有效，长度对应实际可读取范围。
        //    平台复制数据，调用结束后不再依赖 data 的存储。
        unsafe {
            AMediaFormat_setBuffer(
                self.inner.as_ptr(),
                key.as_ptr(),
                data.as_ptr() as *const c_void,
                data_len,
            );
        }
    }

    pub(crate) fn get_string(&mut self, key: &CStr) -> Result<Option<String>, MediaFormatError> {
        let mut raw_value: *const c_char = null();

        // SAFETY:
        // self 持有有效且未释放的 AMediaFormat
        // key 是有效的、以零字符串结尾的 C 字符串。
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
        // 平台保证它指向有效的，以零字节结尾。
        // self 仍然存活，且期间没有再次调用 getString
        let borrowed_value = unsafe { CStr::from_ptr(raw_value) };

        let text = borrowed_value
            .to_str()
            .map_err(|_| MediaFormatError::InvalidUtf8)?;

        let value = text.to_owned();

        Ok(Some(value))
    }

    pub(crate) fn get_i32(&mut self, key: &CStr) -> Option<i32> {
        let mut value = 0i32;

        let found = unsafe { AMediaFormat_getInt32(self.inner.as_ptr(), key.as_ptr(), &mut value) };

        if !found {
            return None;
        }

        Some(value)
    }

    pub(crate) fn get_i64(&mut self, key: &CStr) -> Option<i64> {
        let mut value = 0i64;

        let found = unsafe { AMediaFormat_getInt64(self.inner.as_ptr(), key.as_ptr(), &mut value) };

        if !found {
            return None;
        }

        Some(value)
    }

    pub(crate) fn get_buffer(&mut self, key: &CStr) -> Result<Option<Vec<u8>>, MediaFormatError> {
        let mut raw_data: *mut c_void = null_mut();
        let mut byte_len = 0usize;
        // SAFETY:
        // self 持有有效且未释放的 AMediaFormat。
        // key 是有效的、以零字节结尾的 C 字符串。
        // 两个输出变量的地址在调用期间有效且可写。
        let found = unsafe {
            AMediaFormat_getBuffer(
                self.inner.as_ptr(),
                key.as_ptr(),
                &mut raw_data,
                &mut byte_len,
            )
        };

        if !found {
            return Ok(None);
        }

        if byte_len == 0 {
            return Ok(Some(Vec::new()));
        }

        if raw_data.is_null() {
            return Err(MediaFormatError::NullBufferPointer);
        }

        if byte_len > isize::MAX as usize {
            return Err(MediaFormatError::InvalidBufferSize { size: byte_len });
        }

        // SAFETY:
        // NDK 成功返回的缓冲区包含 byte_len 个已初始化、可读取的字节，
        // 位于同一分配中，范围不会越过地址空间。
        // 指针已确认非空，长度不超过 isize::MAX，u8 的对齐要求为 1.
        // 格式对象仍然存活，借用期间没有修改或释放该字段。
        let borrowed_data = unsafe { std::slice::from_raw_parts(raw_data as *const u8, byte_len) };

        let data = borrowed_data.to_vec();
        Ok(Some(data))
    }

    pub(crate) fn as_ptr(&self) -> *const AMediaFormat {
        self.inner.as_ptr()
    }
}

impl Drop for NativeMediaFormat {
    fn drop(&mut self) {
        unsafe { AMediaFormat_delete(self.inner.as_ptr()) };
    }
}
