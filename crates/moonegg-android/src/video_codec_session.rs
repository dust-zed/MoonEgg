use std::{cell::RefCell, rc::Rc};

use crate::media_codec::{MediaCodecError, NativeMediaCodec, SurfaceOutputToken};

#[derive(Debug, thiserror::Error)]
pub(crate) enum VideoCodecSessionError {
    #[error("视频编解码调用失败: {source}")]
    Codec { source: MediaCodecError },
    #[error("视频解码器正在被其他操作借用")]
    BorrowConflict,
}

/// 解码器和视频帧共同持有 codec 管理对象
#[derive(Debug)]
pub(crate) struct VideoCodecSession {
    codec: RefCell<NativeMediaCodec>,
    // 帧析构时，如果 codec 正在被借用，
    // 先保存需要丢弃的 token，等借用结束后处理。
    pending_discards: RefCell<Vec<SurfaceOutputToken>>,
    // Drop 不能返回错误，因此保存清理错误，
    // 由下一次正常操作报告给调用方。
    cleanup_error: RefCell<Option<MediaCodecError>>,
}

impl VideoCodecSession {
    pub(crate) fn new(codec: NativeMediaCodec) -> Rc<Self> {
        Rc::new(Self {
            codec: RefCell::new(codec),
            pending_discards: RefCell::new(Vec::new()),
            cleanup_error: RefCell::new(None),
        })
    }

    /// 所有正常的 codec 操作都经过这个入口。
    ///
    /// 借用只持续到 operation 返回，不保存在视频帧中
    pub(crate) fn with_codec<T>(
        &self,
        operation: impl FnOnce(&mut NativeMediaCodec) -> Result<T, MediaCodecError>,
    ) -> Result<T, VideoCodecSessionError> {
        // 先报告此前帧析构过程中发生的清理错误
        if let Some(source) = self.cleanup_error.borrow_mut().take() {
            return Err(VideoCodecSessionError::Codec { source });
        }

        // 使用 try_borrow_mut，避免借用冲突直接导致 panic
        let mut codec = self
            .codec
            .try_borrow_mut()
            .map_err(|_| VideoCodecSessionError::BorrowConflict)?;
        // 在进行新操作之前，先处理之前延迟的丢弃
        self.drain_pending_discards(&mut codec)
            .map_err(|source| VideoCodecSessionError::Codec { source })?;

        let result = operation(&mut codec);

        // operation 执行期间也可能销毁视频帧。
        // 那些帧的 Drop 会把 token 放进 pending_discards,
        // 因此操作结束后再清理一次
        let cleanup_result = self.drain_pending_discards(&mut codec);

        match (result, cleanup_result) {
            // 正常操作本身失败时，优先报告该错误
            (Err(source), _) => Err(VideoCodecSessionError::Codec { source }),
            // 正常操作成功，但随后清理失败
            (Ok(_), Err(source)) => Err(VideoCodecSessionError::Codec { source }),
            (Ok(value), Ok(())) => Ok(value),
        }
    }

    fn drain_pending_discards(&self, codec: &mut NativeMediaCodec) -> Result<(), MediaCodecError> {
        // 先取走整个队列，立即结束对队列的可变借用。
        // 后续操作期间，新的析构仍可以向原队列添加 token
        let pending_discards = {
            let mut queue = self.pending_discards.borrow_mut();
            std::mem::take(&mut *queue)
        };

        for token in pending_discards {
            // flush 后已经失效的 token 会由底层安全忽略。
            codec.release_surface_output(token, false)?;
        }
        Ok(())
    }
    /// 专供视频帧的 Drop 调用，不返回错误，也不要求立即接到 codec。
    pub(crate) fn discard_on_drop(&self, token: SurfaceOutputToken) {
        // 先保存 token，确保借用冲突是仍保留清理责任。
        self.pending_discards.borrow_mut().push(token);

        // 借得到 codec：立即清理
        // 借不到： 等待当前 with_codec 操作结束后的清理
        let result = self.with_codec(|_| Ok(()));

        match result {
            Ok(()) => {}

            Err(VideoCodecSessionError::BorrowConflict) => {
                // token 仍在 pending_discards 中
            }

            Err(VideoCodecSessionError::Codec { source }) => {
                // Drop 无法返回错误，留给下一次正常操作报告。
                *self.cleanup_error.borrow_mut() = Some(source)
            }
        }
    }
}
