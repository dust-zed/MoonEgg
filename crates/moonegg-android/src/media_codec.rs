use std::{
    error,
    ffi::CStr,
    ptr::{NonNull, null_mut},
    slice::from_raw_parts,
};

use ndk_sys::{
    AMEDIACODEC_BUFFER_FLAG_CODEC_CONFIG, AMEDIACODEC_BUFFER_FLAG_END_OF_STREAM,
    AMEDIACODEC_INFO_OUTPUT_BUFFERS_CHANGED, AMEDIACODEC_INFO_OUTPUT_FORMAT_CHANGED,
    AMEDIACODEC_INFO_TRY_AGAIN_LATER, AMediaCodec, AMediaCodec_configure,
    AMediaCodec_createDecoderByType, AMediaCodec_delete, AMediaCodec_dequeueInputBuffer,
    AMediaCodec_dequeueOutputBuffer, AMediaCodec_flush, AMediaCodec_getInputBuffer,
    AMediaCodec_getOutputBuffer, AMediaCodec_getOutputFormat, AMediaCodec_queueInputBuffer,
    AMediaCodec_releaseOutputBuffer, AMediaCodec_start, AMediaCodecBufferInfo, media_status_t,
};

use crate::media_format::NativeMediaFormat;

#[derive(Debug, thiserror::Error)]
pub(crate) enum MediaCodecError {
    #[error("创建 MediaCodec 解码器失败")]
    CreateFailed,
    #[error("配置 MediaCodec 失败：status={status}")]
    ConfigureFailed { status: i32 },
    #[error("启动 MediaCodec 失败：status={status}")]
    StartFailed { status: i32 },
    #[error("申请 MediaCodec 输入槽位失败：status={status}")]
    DequeueInputFailed { status: isize },
    #[error("MediaCodec 状态不允许此操作，或所需缓冲区槽位尚未取得")]
    InvalidState,
    #[error("普通编码数据或解码配置不能为空；EOS 应通过专用入口提交")]
    EmptyInput,
    #[error("MediaCodec 输入槽位返回空指针：index={index}")]
    NullInputBuffer { index: usize },
    #[error("MediaCodec 输入缓冲区不足：required={required} 字节，capacity={capacity} 字节")]
    InputBufferTooSmall { required: usize, capacity: usize },
    #[error(
        "MediaCodec 输出有效长度超过缓冲区容量：required={required} 字节，capacity={capacity} 字节"
    )]
    OutputBufferTooSmall { required: usize, capacity: usize },
    #[error("提交 MediaCodec 输入槽位失败：status={status}")]
    QueueInputFailed { status: i32 },
    #[error("获取 MediaCodec 输出槽位失败：status={status}")]
    DequeueOutputFailed { status: isize },
    #[error("获取 MediaCodec 输出格式失败")]
    GetOutputFormatFailed,
    #[error("MediaCodec 输出有效长度不能为负数：size={size} 字节")]
    InvalidOutputSize { size: i32 },
    #[error("MediaCodec 输出槽位返回空指针：index={index}")]
    NullOutputBuffer { index: usize },
    #[error("归还 MediaCodec 输出槽位失败：status={status}")]
    ReleaseOutputFailed { status: i32 },
    #[error("清空 MediaCodec 缓冲区失败：status={status}")]
    FlushFailed { status: i32 },
}

#[derive(Debug)]
pub(crate) enum InputQueueResult {
    Queued,
    WouldBlock,
}

#[derive(Debug)]
pub(crate) enum OutputPoll {
    NotReady,
    FormatChanged,
    BuffersChanged,
    BufferReady,
}

#[derive(Debug)]
struct PendingOutput {
    index: usize,
    info: AMediaCodecBufferInfo,
}

#[derive(Debug)]
pub(crate) struct CodecOutputBuffer {
    data: Vec<u8>,
    presentation_time_us: i64,
    flags: u32,
}

impl CodecOutputBuffer {
    pub(crate) fn into_parts(self) -> (Vec<u8>, i64, u32) {
        (self.data, self.presentation_time_us, self.flags)
    }
}

pub(crate) enum CodecOutput {
    NotReady,
    FormatChanged { format: NativeMediaFormat },
    Buffer { buffer: CodecOutputBuffer },
    EndOfStream,
}

#[derive(Debug)]
pub(crate) struct NativeMediaCodec {
    inner: NonNull<AMediaCodec>,
    pending_input_index: Option<usize>,
    failed: bool,
    input_eos_queued: bool,
    pending_output: Option<PendingOutput>,
    // 已经取得并处理了带 EOS 标记的输出槽位
    output_eos_seen: bool,
    output_started: bool,
}

impl NativeMediaCodec {
    pub(crate) fn new_audio_decoder(
        mime: &CStr,
        format: &NativeMediaFormat,
    ) -> Result<Self, MediaCodecError> {
        // SAFETY: mime 是调用期间有效、以零字节结尾的 C 字符串。
        // 返回的句柄随后检查非空，并由唯一的 NativeMediaCodec 管理。
        let raw_codec = unsafe { AMediaCodec_createDecoderByType(mime.as_ptr()) };

        let inner = NonNull::new(raw_codec).ok_or(MediaCodecError::CreateFailed)?;

        // 目的就是为了从这里开始，任何后续失败都通过 Drop 释放资源
        let codec = Self {
            inner,
            pending_input_index: None,
            failed: false,
            input_eos_queued: false,
            pending_output: None,
            output_eos_seen: false,
            output_started: false,
        };

        // SAFETY: codec 刚创建且尚未配置，format 在调用期间有效。
        // 当前为未加密音频的缓冲区输出模式，Surface 和 Crypto 均允许为空；
        // flags=0 表示解码。此时句柄尚未向外暴露，不存在并发调用。
        let status = unsafe {
            AMediaCodec_configure(inner.as_ptr(), format.as_ptr(), null_mut(), null_mut(), 0)
        };

        if status != media_status_t::AMEDIA_OK {
            return Err(MediaCodecError::ConfigureFailed { status: status.0 });
        }

        // SAFETY: codec 有效且 configure 已成功，尚未启动，无并发访问。
        let status = unsafe { AMediaCodec_start(codec.inner.as_ptr()) };

        if status != media_status_t::AMEDIA_OK {
            return Err(MediaCodecError::StartFailed { status: status.0 });
        }

        Ok(codec)
    }

    // 已获得缓冲区则复用，没有才申请
    fn ensure_input_buffer(&mut self) -> Result<Option<usize>, MediaCodecError> {
        if self.failed || self.input_eos_queued {
            return Err(MediaCodecError::InvalidState);
        }
        if self.pending_input_index.is_some() {
            return Ok(self.pending_input_index);
        }
        // SAFETY:
        // codec 有效、已启动，使用同步模式，当前独占借用 self
        let raw_index = unsafe { AMediaCodec_dequeueInputBuffer(self.inner.as_ptr(), 0) };

        if raw_index == AMEDIACODEC_INFO_TRY_AGAIN_LATER as isize {
            return Ok(None);
        }

        if raw_index < 0 {
            self.failed = true;
            return Err(MediaCodecError::DequeueInputFailed { status: raw_index });
        }

        let input_index = raw_index as usize;
        self.pending_input_index = Some(input_index);
        Ok(self.pending_input_index)
    }

    pub(crate) fn try_queue_data(
        &mut self,
        data: &[u8],
        presentation_time_us: u64,
    ) -> Result<InputQueueResult, MediaCodecError> {
        self.try_queue_bytes(data, presentation_time_us, 0)
    }

    pub(crate) fn try_queue_codec_config(
        &mut self,
        codec_config: &[u8],
    ) -> Result<InputQueueResult, MediaCodecError> {
        self.try_queue_bytes(codec_config, 0, AMEDIACODEC_BUFFER_FLAG_CODEC_CONFIG)
    }

    pub(crate) fn try_queue_eos(&mut self) -> Result<InputQueueResult, MediaCodecError> {
        if self.failed || self.input_eos_queued {
            return Err(MediaCodecError::InvalidState);
        }
        let Some(input_index) = self.ensure_input_buffer()? else {
            return Ok(InputQueueResult::WouldBlock);
        };
        // SAFETY: input_index 是本 codec 已取得且尚未提交的输入槽位。
        // EOS 使用 offset=0、size=0，不读取数据；当前独占访问 codec。
        // 成功后清除 pending_input_index，避免重复提交该槽位。
        let status = unsafe {
            AMediaCodec_queueInputBuffer(
                self.inner.as_ptr(),
                input_index,
                0,
                0,
                0,
                AMEDIACODEC_BUFFER_FLAG_END_OF_STREAM,
            )
        };

        if status != media_status_t::AMEDIA_OK {
            self.failed = true;
            return Err(MediaCodecError::QueueInputFailed { status: status.0 });
        }
        self.pending_input_index = None;
        self.input_eos_queued = true;
        Ok(InputQueueResult::Queued)
    }

    fn try_queue_bytes(
        &mut self,
        data: &[u8],
        presentation_time_us: u64,
        flags: u32,
    ) -> Result<InputQueueResult, MediaCodecError> {
        if self.failed || self.input_eos_queued {
            return Err(MediaCodecError::InvalidState);
        }
        if data.is_empty() {
            return Err(MediaCodecError::EmptyInput);
        }
        let Some(input_index) = self.ensure_input_buffer()? else {
            return Ok(InputQueueResult::WouldBlock);
        };
        let mut buffer_capacity = 0usize;
        // SAFETY: input_index 是当前持有、尚未提交的输入槽位，codec 有效。
        // buffer_capacity 是调用期间有效、可写的 usize 输出位置。
        let raw_buffer = unsafe {
            AMediaCodec_getInputBuffer(self.inner.as_ptr(), input_index, &mut buffer_capacity)
        };

        if raw_buffer.is_null() {
            self.failed = true;
            return Err(MediaCodecError::NullInputBuffer { index: input_index });
        }

        if data.len() > buffer_capacity {
            return Err(MediaCodecError::InputBufferTooSmall {
                required: data.len(),
                capacity: buffer_capacity,
            });
        }

        // SAFETY: raw_buffer 已确认非空，平台允许在提交前写入该输入槽位；
        // data.len() 不超过平台报告的容量，源切片包含同样多的有效字节。
        // 源数据由调用方持有，与 codec 私有输入缓冲区不重叠，u8 对齐要求为 1。
        // 当前独占访问 codec，复制期间不会提交、flush 或释放槽位。
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), raw_buffer, data.len());
        };

        // SAFETY: 当前持有该输入槽位，offset=0、size=data.len() 均在已检查的容量内，
        // 对应字节已初始化。提交之后不再使用 raw_buffer，并在成功后清除槽位索引。
        let status = unsafe {
            AMediaCodec_queueInputBuffer(
                self.inner.as_ptr(),
                input_index,
                0,
                data.len(),
                presentation_time_us,
                flags,
            )
        };
        if status != media_status_t::AMEDIA_OK {
            self.failed = true;
            return Err(MediaCodecError::QueueInputFailed { status: status.0 });
        }

        self.pending_input_index = None;
        Ok(InputQueueResult::Queued)
    }

    fn ensure_output_buffer(&mut self) -> Result<OutputPoll, MediaCodecError> {
        if self.failed {
            return Err(MediaCodecError::InvalidState);
        }
        if self.pending_output.is_some() {
            return Ok(OutputPoll::BufferReady);
        }

        let mut buffer_info = AMediaCodecBufferInfo {
            offset: 0,
            size: 0,
            presentationTimeUs: 0,
            flags: 0,
        };
        // SAFETY: codec 已启动且使用同步模式，当前独占访问。
        // buffer_info 是调用期间有效、对齐且可写的输出结构；timeout=0 不阻塞等待。
        let raw_index =
            unsafe { AMediaCodec_dequeueOutputBuffer(self.inner.as_ptr(), &mut buffer_info, 0) };
        if raw_index == AMEDIACODEC_INFO_TRY_AGAIN_LATER as isize {
            return Ok(OutputPoll::NotReady);
        }
        if raw_index == AMEDIACODEC_INFO_OUTPUT_FORMAT_CHANGED as isize {
            self.output_started = true;
            return Ok(OutputPoll::FormatChanged);
        }
        if raw_index == AMEDIACODEC_INFO_OUTPUT_BUFFERS_CHANGED as isize {
            return Ok(OutputPoll::BuffersChanged);
        }
        if raw_index < 0 {
            self.failed = true;
            return Err(MediaCodecError::DequeueOutputFailed { status: raw_index });
        }
        let output_index = raw_index as usize;
        self.output_started = true;
        self.pending_output = Some(PendingOutput {
            index: output_index,
            info: buffer_info,
        });
        Ok(OutputPoll::BufferReady)
    }

    fn output_format(&mut self) -> Result<NativeMediaFormat, MediaCodecError> {
        if self.failed {
            return Err(MediaCodecError::InvalidState);
        }

        // SAFETY: codec 有效且未处于失败状态；当前独占访问。
        // 平台返回独立的格式对象，所有权由调用方接管。
        let raw_format = unsafe { AMediaCodec_getOutputFormat(self.inner.as_ptr()) };
        // SAFETY: raw_format 是本次调用返回的新拥有对象或空指针。
        // 此处唯一接管所有权，不会通过原始指针另行释放。
        let format = unsafe { NativeMediaFormat::from_owned_raw(raw_format) }.ok_or_else(|| {
            self.failed = true;
            MediaCodecError::GetOutputFormatFailed
        })?;
        Ok(format)
    }

    fn copy_output_data(
        &self,
        output_index: usize,
        info: &AMediaCodecBufferInfo,
    ) -> Result<Vec<u8>, MediaCodecError> {
        let byte_len = usize::try_from(info.size)
            .map_err(|_| MediaCodecError::InvalidOutputSize { size: info.size })?;
        if byte_len == 0 {
            return Ok(vec![]);
        }

        let mut reported_capacity = 0usize;
        // SAFETY: output_index 来自 pending_output，已出队且尚未归还或被 flush 失效。
        // codec 仍存活，reported_capacity 是有效、可写的输出位置。
        let raw_buffer = unsafe {
            AMediaCodec_getOutputBuffer(self.inner.as_ptr(), output_index, &mut reported_capacity)
        };
        if raw_buffer.is_null() {
            return Err(MediaCodecError::NullOutputBuffer {
                index: output_index,
            });
        }
        // 容量用于检查边界；本次有效长度决定复制范围。
        if byte_len > reported_capacity {
            return Err(MediaCodecError::OutputBufferTooSmall {
                required: byte_len,
                capacity: reported_capacity,
            });
        }
        // SAFETY: 指针已确认非空，byte_len 不超过平台返回的容量；平台保证已出队
        // 输出中的有效字节已初始化、位于同一缓冲区内，u8 对齐要求为 1。
        // byte_len 来自非负 i32，在支持的 Android 32/64 位目标上不超过 isize::MAX。
        // 复制完成前不会归还槽位、flush 或释放 codec；返回 Vec 后不保留平台内存借用。
        let buffer = unsafe { from_raw_parts(raw_buffer, byte_len) }.to_vec();
        Ok(buffer)
    }

    fn take_output_buffer(&mut self) -> Result<CodecOutputBuffer, MediaCodecError> {
        if self.failed {
            return Err(MediaCodecError::InvalidState);
        }
        let Some(pending_output) = self.pending_output.as_ref() else {
            return Err(MediaCodecError::InvalidState);
        };
        let (output_index, buffer_info) = (pending_output.index, pending_output.info);
        let copy_result = self.copy_output_data(output_index, &buffer_info);

        // SAFETY: output_index 是当前尚未归还的输出槽位，复制操作已经结束，
        // 不再持有其内存借用；render=false 适用于当前音频缓冲区输出模式。
        // 成功后清除 pending_output；失败则将 codec 标记为不可继续使用。
        let status =
            unsafe { AMediaCodec_releaseOutputBuffer(self.inner.as_ptr(), output_index, false) };
        if status != media_status_t::AMEDIA_OK {
            self.failed = true;
            return Err(MediaCodecError::ReleaseOutputFailed { status: status.0 });
        }
        self.pending_output = None;
        let data = match copy_result {
            Ok(data) => data,
            Err(error) => {
                self.failed = true;
                return Err(error);
            }
        };
        Ok(CodecOutputBuffer {
            data,
            presentation_time_us: buffer_info.presentationTimeUs,
            flags: buffer_info.flags,
        })
    }

    pub(crate) fn has_output_started(&self) -> bool {
        self.output_started
    }

    pub(crate) fn try_receive(&mut self) -> Result<CodecOutput, MediaCodecError> {
        if self.failed {
            return Err(MediaCodecError::InvalidState);
        }
        if self.output_eos_seen {
            return Ok(CodecOutput::EndOfStream);
        }
        match self.ensure_output_buffer()? {
            OutputPoll::NotReady => Ok(CodecOutput::NotReady),
            OutputPoll::BuffersChanged => Ok(CodecOutput::NotReady),
            OutputPoll::FormatChanged => Ok(CodecOutput::FormatChanged {
                format: self.output_format()?,
            }),
            OutputPoll::BufferReady => {
                let output_buffer = self.take_output_buffer()?;
                let has_eos = (output_buffer.flags & AMEDIACODEC_BUFFER_FLAG_END_OF_STREAM) != 0;
                if has_eos {
                    self.output_eos_seen = true;
                }
                if !output_buffer.data.is_empty() {
                    Ok(CodecOutput::Buffer {
                        buffer: output_buffer,
                    })
                } else if has_eos {
                    Ok(CodecOutput::EndOfStream)
                } else {
                    Ok(CodecOutput::NotReady)
                }
            }
        }
    }

    pub(crate) fn flush(&mut self) -> Result<(), MediaCodecError> {
        if self.failed {
            return Err(MediaCodecError::InvalidState);
        }
        // SAFETY: codec 已启动且当前独占访问，没有跨调用保存的平台内存引用。
        // flush 会使已取得的槽位失效，成功后立即清空对应索引及 EOS 状态。
        let status = unsafe { AMediaCodec_flush(self.inner.as_ptr()) };
        if status != media_status_t::AMEDIA_OK {
            self.failed = true;
            return Err(MediaCodecError::FlushFailed { status: status.0 });
        }
        self.pending_input_index = None;
        self.pending_output = None;
        self.input_eos_queued = false;
        self.output_eos_seen = false;
        Ok(())
    }
}

impl Drop for NativeMediaCodec {
    fn drop(&mut self) {
        // SAFETY：当前实例唯一拥有有效句柄，此后不再使用。
        unsafe { AMediaCodec_delete(self.inner.as_ptr()) };
    }
}
