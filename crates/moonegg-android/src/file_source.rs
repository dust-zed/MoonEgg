use std::{
    fs::File,
    io::{self, ErrorKind},
    os::fd::FromRawFd,
};

/// 复制调用方的文件描述符，返回由 Rust 独立管理生命周期的 File
/// SAFETY:
/// 调用方必须保证原 fd 在本次调用期间保持有效，且不会被并发关闭
/// 本函数不会关闭原 fd
/// 返回的文件与原 fd 共享底层文件偏移，不能并发读取或定位。
pub fn duplicate_file_descriptor(fd: i32) -> io::Result<File> {
    if fd < 0 {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "file descriptor must be non-negative",
        ));
    }

    let duplicate_fd = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };

    if duplicate_fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_fd(duplicate_fd) };
    Ok(file)
}
