use std::{
    fs::File,
    io::{self, ErrorKind},
    os::fd::FromRawFd,
};

/// 复制调用方的文件描述符，返回由 Rust 独立管理生命周期的 File。
///
/// 为确保复制的是预期文件，调用方应保证原 fd 在调用期间不会被关闭或复用。
/// 本函数不会关闭原 fd；成功返回后，调用方可以关闭自己的 fd。
/// 返回的文件与原 fd 共享底层文件偏移，使用依赖该偏移的读写或定位操作时需要协调。
pub fn duplicate_file_descriptor(fd: i32) -> io::Result<File> {
    if fd < 0 {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "file descriptor must be non-negative",
        ));
    }

    // SAFETY: F_DUPFD_CLOEXEC 的第三个参数是整数下限 0，不涉及用户内存指针。
    // 内核检查 fd 的有效性；成功时返回一个新的、尚未由 Rust 对象持有的 fd。
    let duplicate_fd = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };

    if duplicate_fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: duplicate_fd 来自成功的 fcntl，当前有效且只有此处接管所有权。
    // 之后由 File 负责关闭，不再手动关闭或构造第二个拥有者。
    let file = unsafe { File::from_raw_fd(duplicate_fd) };
    Ok(file)
}
