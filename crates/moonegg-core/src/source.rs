use std::{
    fs::File,
    io::{self, ErrorKind, Read, Seek, SeekFrom},
    path::PathBuf,
};

pub(crate) struct BoundedReader<R> {
    inner: R,
    start: u64,
    length: u64,
    position: u64,
}

impl<R: Seek> BoundedReader<R> {
    pub(crate) fn new(mut inner: R, start: u64, length: u64) -> io::Result<Self> {
        let end = start
            .checked_add(length)
            .ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "source range overflows"))?;

        let source_len = inner.seek(SeekFrom::End(0))?;

        if end > source_len {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "source range exceeds input length",
            ));
        }
        inner.seek(SeekFrom::Start(start))?;

        Ok(Self {
            inner,
            start,
            length,
            position: 0,
        })
    }
}

impl<R: Seek> Seek for BoundedReader<R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let target_position = match pos {
            SeekFrom::Start(offset) => i128::from(offset),
            SeekFrom::Current(offset) => i128::from(self.position) + i128::from(offset),
            SeekFrom::End(offset) => i128::from(self.length) + i128::from(offset),
        };
        if target_position > i128::from(self.length) || target_position < 0 {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "seek position is outside source range",
            ));
        }

        let next_position = u64::try_from(target_position).map_err(|_| {
            io::Error::new(ErrorKind::InvalidInput, "seek target position overflow")
        })?;

        let absolute_position = self.start + next_position;
        self.inner.seek(SeekFrom::Start(absolute_position))?;

        self.position = next_position;
        Ok(next_position)
    }
}

impl<R: Seek + Read> Read for BoundedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() || self.position == self.length {
            return Ok(0);
        }

        let remaining = self.length - self.position;
        let read_limit = usize::try_from(remaining).unwrap_or(usize::MAX);
        let read_len = buf.len().min(read_limit);
        let absolute_position = self.start + self.position;
        self.inner.seek(SeekFrom::Start(absolute_position))?;

        let bytes_read = self.inner.read(&mut buf[..read_len])?;

        self.position += bytes_read as u64;
        Ok(bytes_read)
    }
}

pub enum FileSource {
    Path(PathBuf),
    Region { file: File, start: u64, length: u64 },
}

impl FileSource {
    pub(crate) fn open_reader(&self) -> io::Result<BoundedReader<File>> {
        match self {
            FileSource::Path(path) => {
                let file = File::open(path)?;
                let metadata = file.metadata()?;
                let length = metadata.len();
                BoundedReader::new(file, 0, length)
            }
            FileSource::Region {
                file,
                start,
                length,
            } => {
                let playback_file = file.try_clone()?;
                BoundedReader::new(playback_file, *start, *length)
            }
        }
    }
}
