use std::{
    error,
    ffi::CStr,
    ptr::{NonNull, null_mut},
    slice::from_raw_parts,
};

use ndk_sys::{
    AMEDIACODEC_BUFFER_FLAG_END_OF_STREAM, AMEDIACODEC_INFO_OUTPUT_BUFFERS_CHANGED,
    AMEDIACODEC_INFO_OUTPUT_FORMAT_CHANGED, AMEDIACODEC_INFO_TRY_AGAIN_LATER, AMediaCodec,
    AMediaCodec_configure, AMediaCodec_createDecoderByType, AMediaCodec_delete,
    AMediaCodec_dequeueInputBuffer, AMediaCodec_dequeueOutputBuffer, AMediaCodec_getInputBuffer,
    AMediaCodec_getOutputBuffer, AMediaCodec_getOutputFormat, AMediaCodec_queueInputBuffer,
    AMediaCodec_releaseOutputBuffer, AMediaCodec_start, AMediaCodecBufferInfo, media_status_t,
};

use crate::media_format::NativeMediaFormat;

#[derive(Debug, thiserror::Error)]
pub(crate) enum MediaCodecError {
    #[error("")]
    CreateFailed,
    #[error("")]
    ConfigureFailed { status: i32 },
    #[error("")]
    StartFailed { status: i32 },
    #[error("")]
    DequeueInputFailed { status: isize },
    #[error("")]
    InvalidState,
    #[error("")]
    EmptyInput,
    #[error("")]
    NullInputBuffer { index: usize },
    #[error("")]
    InputBufferTooSmall { required: usize, capacity: usize },
    #[error("")]
    QueueInputFailed { status: i32 },
    #[error("")]
    DequeueOutputFailed { status: isize },
    #[error("")]
    GetOutputFormatFailed,
    #[error("")]
    InvalidOutputSize { size: i32 },
    #[error("")]
    NullOutputBuffer { index: usize },
    #[error("")]
    ReleaseOutputFailed { status: i32 },
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
}

impl NativeMediaCodec {
    pub(crate) fn new_audio_decoder(
        mime: &CStr,
        format: &NativeMediaFormat,
    ) -> Result<Self, MediaCodecError> {
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
        };

        let status = unsafe {
            AMediaCodec_configure(inner.as_ptr(), format.as_ptr(), null_mut(), null_mut(), 0)
        };

        if status != media_status_t::AMEDIA_OK {
            return Err(MediaCodecError::ConfigureFailed { status: status.0 });
        }

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
        // SAFETY:
        // data 是有效的源切片
        // raw_buffer 属于当前已取得、尚未提交的输入槽位
        // 已确认指针非空，且容量足以容纳 data.len() 个字节。
        // 源数据与平台缓冲区不重叠， u8 的对齐要求为 1
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), raw_buffer, data.len());
        };
        let status = unsafe {
            AMediaCodec_queueInputBuffer(
                self.inner.as_ptr(),
                input_index,
                0,
                data.len(),
                presentation_time_us,
                0,
            )
        };
        if status != media_status_t::AMEDIA_OK {
            self.failed = true;
            return Err(MediaCodecError::QueueInputFailed { status: status.0 });
        }
        self.pending_input_index = None;
        Ok(InputQueueResult::Queued)
    }

    pub(crate) fn try_queue_eos(&mut self) -> Result<InputQueueResult, MediaCodecError> {
        if self.failed || self.input_eos_queued {
            return Err(MediaCodecError::InvalidState);
        }
        let Some(input_index) = self.ensure_input_buffer()? else {
            return Ok(InputQueueResult::WouldBlock);
        };
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
        let raw_index =
            unsafe { AMediaCodec_dequeueOutputBuffer(self.inner.as_ptr(), &mut buffer_info, 0) };
        if raw_index == AMEDIACODEC_INFO_TRY_AGAIN_LATER as isize {
            return Ok(OutputPoll::NotReady);
        }
        if raw_index == AMEDIACODEC_INFO_OUTPUT_FORMAT_CHANGED as isize {
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

        let raw_format = unsafe { AMediaCodec_getOutputFormat(self.inner.as_ptr()) };
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

        let mut reported_size = 0usize;
        let raw_buffer = unsafe {
            AMediaCodec_getOutputBuffer(self.inner.as_ptr(), output_index, &mut reported_size)
        };
        if raw_buffer.is_null() {
            return Err(MediaCodecError::NullOutputBuffer {
                index: output_index,
            });
        }
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
}

impl Drop for NativeMediaCodec {
    fn drop(&mut self) {
        // SAFETY：当前实例唯一拥有有效句柄，此后不再使用。
        unsafe { AMediaCodec_delete(self.inner.as_ptr()) };
    }
}
