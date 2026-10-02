use std::rc::Rc;

use crate::{
    media_codec::{MediaCodecError, SurfaceOutputToken},
    video_codec_session::{VideoCodecSession, VideoCodecSessionError},
};

pub(crate) struct AndroidVideoBuffer {
    session: Rc<VideoCodecSession>,
    token: Option<SurfaceOutputToken>,
}

impl AndroidVideoBuffer {
    pub(crate) fn new(session: Rc<VideoCodecSession>, token: SurfaceOutputToken) -> Self {
        Self {
            session,
            token: Some(token),
        }
    }

    pub(crate) fn present(self) -> Result<(), VideoCodecSessionError> {
        self.finish(true)
    }

    pub(crate) fn discard(self) -> Result<(), VideoCodecSessionError> {
        self.finish(false)
    }

    fn finish(mut self, render: bool) -> Result<(), VideoCodecSessionError> {
        let session = Rc::clone(&self.session);
        session.with_codec(|codec| {
            let token = self.token.take();
            match token {
                None => Err(MediaCodecError::InvalidState),
                Some(token) => codec.release_surface_output(token, render),
            }
        })
    }
}

impl Drop for AndroidVideoBuffer {
    fn drop(&mut self) {
        let Some(token) = self.token.take() else {
            return;
        };
        self.session.discard_on_drop(token);
    }
}
