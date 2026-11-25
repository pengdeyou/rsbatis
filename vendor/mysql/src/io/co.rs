//! 可插拔的 socket 后端：把 `net::TcpStream` 的读写交给外部实现。
//!
//! 这是这个 fork 唯一存在的理由。上游把 `Stream` 做成了封闭枚举，
//! 里面硬编码 `BufStream<net::TcpStream>`，外部没有任何注入点——
//! 于是"数据库读写让出 worker"这件事在外部做不到。
//!
//! 协议逻辑一行没动：握手、认证、结果集解析、类型映射全部保持上游原样。
//! 换掉的只是最底层的搬运工。

use std::io::{self, Read, Write};
use std::net;

/// 读一个段落。返回 `Ok(0)` 表示 EOF。
pub type ReadHook = fn(&net::TcpStream, &mut [u8]) -> io::Result<usize>;
/// 写一次。允许部分写入，调用方（`BufStream`）会继续写剩下的。
pub type WriteHook = fn(&net::TcpStream, &[u8]) -> io::Result<usize>;

fn read_hook() -> Option<ReadHook> {
    READ_HOOK.get().copied()
}

fn write_hook() -> Option<WriteHook> {
    WRITE_HOOK.get().copied()
}

static READ_HOOK: std::sync::OnceLock<ReadHook> = std::sync::OnceLock::new();
static WRITE_HOOK: std::sync::OnceLock<WriteHook> = std::sync::OnceLock::new();

/// 装读写钩子。只能装一次；不装就走阻塞 IO（与上游行为完全一致）。
pub fn set_io_hooks(read: ReadHook, write: WriteHook) {
    let _ = READ_HOOK.set(read);
    let _ = WRITE_HOOK.set(write);
}

/// 走钩子建立过多少条连接。只用于自证"这条路真的被选中了"。
pub static CO_CONNECTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// 读到的字节计数（自证读写确实走了钩子）。
pub static CO_READ_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
pub static CO_WRITE_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub(crate) fn co_read(s: &net::TcpStream, buf: &mut [u8]) -> io::Result<usize> {
    CO_READ_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    match read_hook() {
        Some(f) => f(s, buf),
        // 没装钩子：退化成阻塞读，等价于上游
        None => (&*s).read(buf),
    }
}

pub(crate) fn co_write(s: &net::TcpStream, buf: &[u8]) -> io::Result<usize> {
    CO_WRITE_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    match write_hook() {
        Some(f) => f(s, buf),
        None => (&*s).write(buf),
    }
}

/// 把 `TcpStream` 包成一个"读写走钩子"的 socket。
#[derive(Debug)]
pub struct CoSocket(pub net::TcpStream);

impl CoSocket {
    pub fn get_ref(&self) -> &net::TcpStream {
        &self.0
    }
}

impl Read for CoSocket {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        co_read(&self.0, buf)
    }
}

impl Write for CoSocket {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        co_write(&self.0, buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
