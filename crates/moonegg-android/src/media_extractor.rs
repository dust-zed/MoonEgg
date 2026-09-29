use std::{fs::File, os::fd::AsRawFd, ptr::NonNull};

use ndk_sys::{
    AMediaExtractor_advance, AMediaExtractor_delete, AMediaExtractor_getSampleFlags,
    AMediaExtractor_getSampleSize, AMediaExtractor_getSampleTime,
    AMediaExtractor_getSampleTrackIndex, AMediaExtractor_getTrackCount,
    AMediaExtractor_getTrackFormat, AMediaExtractor_new, AMediaExtractor_readSampleData,
    AMediaExtractor_seekTo, AMediaExtractor_selectTrack, AMediaExtractor_setDataSourceFd, SeekMode,
    media_status_t,
};

use crate::media_format::NativeMediaFormat;

#[derive(Debug, thiserror::Error)]
pub(crate) enum MediaExtractorError {
    #[error("非法的文件范围, start {start:?}, length {length:?}")]
    InvalidSourceRange { start: u64, length: u64 },
    #[error("创建 MediaExtractor 失败")]
    CreateFailed,
    #[error("设置数据源出错： {status:?}")]
    SetDataSourceFailed { status: i32 },
    #[error("索引传递错误, track_index: {track_index}, track_count: {track_count}")]
    InvalidTrackIndex {
        track_index: usize,
        track_count: usize,
    },
    #[error("未能取得Track format: {track_index}")]
    GetTrackFormatFailed { track_index: usize },
    #[error("选择媒体轨道失败：track_index={track_index}, status={status}")]
    SelectTrackFailed { track_index: usize, status: i32 },
    #[error("读取当前编码样本失败，或已无可读取的样本")]
    ReadSampleFailed,
    #[error("样本读取长度超过目标缓冲区容量：size={size} 字节，capacity={capacity} 字节")]
    InvalidReadSize { size: usize, capacity: usize },
    #[error("seek 目标不能为负数：target_us={target_us} μs")]
    InvalidSeekTarget { target_us: i64 },
    #[error("MediaExtractor seek 失败：target_us={target_us} μs, status={status}")]
    SeekFailed { target_us: i64, status: i32 },
}

pub(crate) struct NativeMediaExtractor {
    inner: NonNull<ndk_sys::AMediaExtractor>,
    file: File,
}

impl NativeMediaExtractor {
    pub(crate) fn from_file(
        file: File,
        start: u64,
        length: u64,
    ) -> Result<Self, MediaExtractorError> {
        let invalid_range = || MediaExtractorError::InvalidSourceRange { start, length };
        let range_end = start.checked_add(length).ok_or_else(invalid_range)?;
        if range_end > i64::MAX as u64 {
            return Err(invalid_range());
        }
        let offset = start as i64;
        let size = length as i64;

        // SAFETY: 此创建函数没有指针参数；返回值随后检查非空并交给 Drop 管理。
        let raw = unsafe { AMediaExtractor_new() };
        let inner = NonNull::new(raw).ok_or(MediaExtractorError::CreateFailed)?;

        let extractor = Self { inner, file };

        // SAFETY: inner 来自成功创建的 extractor，尚未释放
        // file 持有有效 fd，offset 和 size 已通过范围检查。
        let status = unsafe {
            AMediaExtractor_setDataSourceFd(
                extractor.inner.as_ptr(),
                extractor.file.as_raw_fd(),
                offset,
                size,
            )
        };
        if status != media_status_t::AMEDIA_OK {
            return Err(MediaExtractorError::SetDataSourceFailed { status: status.0 });
        }

        Ok(extractor)
    }

    pub(crate) fn track_count(&self) -> usize {
        // SAFETY: self 持有尚未释放的 extractor；句柄不暴露给外部，
        // 查询期间没有其他线程或别名修改、释放它。
        unsafe { AMediaExtractor_getTrackCount(self.inner.as_ptr()) }
    }

    pub(crate) fn track_format(
        &self,
        track_index: usize,
    ) -> Result<NativeMediaFormat, MediaExtractorError> {
        let track_count = self.track_count();

        if track_count <= track_index {
            return Err(MediaExtractorError::InvalidTrackIndex {
                track_index,
                track_count,
            });
        }

        // SAFETY: extractor 有效，track_index 已检查小于轨道总数。
        // 平台返回的格式对象由调用方负责释放。
        let raw_format =
            unsafe { AMediaExtractor_getTrackFormat(self.inner.as_ptr(), track_index) };

        // SAFETY: raw_format 是本次查询返回的新拥有对象或空指针；
        // 此处只移交一次所有权，之后由 NativeMediaFormat 释放。
        let track_format = unsafe { NativeMediaFormat::from_owned_raw(raw_format) }
            .ok_or(MediaExtractorError::GetTrackFormatFailed { track_index })?;

        Ok(track_format)
    }

    pub(crate) fn select_track(&mut self, track_index: usize) -> Result<(), MediaExtractorError> {
        let track_count = self.track_count();
        if track_index >= track_count {
            return Err(MediaExtractorError::InvalidTrackIndex {
                track_index,
                track_count,
            });
        }

        // SAFETY:
        // extractor 有效且未释放，索引已检查；
        // 当前方法独占借用 self，负责修改轨道选择状态
        let status = unsafe { AMediaExtractor_selectTrack(self.inner.as_ptr(), track_index) };

        if status != media_status_t::AMEDIA_OK {
            return Err(MediaExtractorError::SelectTrackFailed {
                track_index,
                status: status.0,
            });
        }
        Ok(())
    }

    pub(crate) fn sample_track_index(&self) -> Option<usize> {
        // SAFETY:
        // extractor 有效且未释放
        let raw_track_index = unsafe { AMediaExtractor_getSampleTrackIndex(self.inner.as_ptr()) };

        if raw_track_index < 0 {
            return None;
        }

        Some(raw_track_index as usize)
    }

    pub(crate) fn sample_size(&self) -> Option<usize> {
        // SAFETY:
        // extractor 有效且未释放
        let raw_sample_size = unsafe { AMediaExtractor_getSampleSize(self.inner.as_ptr()) };
        usize::try_from(raw_sample_size).ok()
    }

    pub(crate) fn read_sample_data(
        &mut self,
        buffer: &mut [u8],
    ) -> Result<usize, MediaExtractorError> {
        // SAFETY:
        // extractor 有效；
        // buffer 在调用期间提供独占且有效的可写内存，
        // 传入的容量不超过切片的实际长度。
        let raw_bytes_read = unsafe {
            AMediaExtractor_readSampleData(self.inner.as_ptr(), buffer.as_mut_ptr(), buffer.len())
        };

        if raw_bytes_read < 0 {
            return Err(MediaExtractorError::ReadSampleFailed);
        }

        let bytes_read = raw_bytes_read as usize;
        if bytes_read > buffer.len() {
            return Err(MediaExtractorError::InvalidReadSize {
                size: bytes_read,
                capacity: buffer.len(),
            });
        }
        Ok(bytes_read)
    }

    pub(crate) fn sample_time_us(&self) -> Option<i64> {
        // SAFETY：
        // extractor 有效且未释放
        let raw_time_us = unsafe { AMediaExtractor_getSampleTime(self.inner.as_ptr()) };

        if raw_time_us == -1 {
            return None;
        }

        Some(raw_time_us)
    }

    pub(crate) fn sample_flags(&self) -> u32 {
        // SAFETY:
        // extractor 有效且未释放
        unsafe { AMediaExtractor_getSampleFlags(self.inner.as_ptr()) }
    }

    pub(crate) fn advance(&mut self) -> bool {
        // SAFETY:
        // extractor 有效，当前方法独占借用 self。
        unsafe { AMediaExtractor_advance(self.inner.as_ptr()) }
    }

    pub(crate) fn seek_to_us(&mut self, target_us: i64) -> Result<(), MediaExtractorError> {
        if target_us < 0 {
            return Err(MediaExtractorError::InvalidSeekTarget { target_us });
        }

        // SAFETY:
        // extractor 有效且未释放
        // 当前独占借用 self，
        let status = unsafe {
            AMediaExtractor_seekTo(
                self.inner.as_ptr(),
                target_us,
                SeekMode::AMEDIAEXTRACTOR_SEEK_PREVIOUS_SYNC,
            )
        };
        if status != media_status_t::AMEDIA_OK {
            return Err(MediaExtractorError::SeekFailed {
                target_us,
                status: status.0,
            });
        }
        Ok(())
    }
}

impl Drop for NativeMediaExtractor {
    fn drop(&mut self) {
        // SAFETY: 本对象唯一拥有有效 extractor，当前没有进行中的平台调用。
        // 持有的 File 此时仍存活；删除 extractor 后再由字段析构关闭文件。
        unsafe { AMediaExtractor_delete(self.inner.as_ptr()) };
    }
}
