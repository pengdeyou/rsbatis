//! 证明 `mysql+co://` 这条路径真的被选中，而不是"看起来配了、实际还是阻塞"。
//!
//! 没做完整握手：握手是上游协议逻辑，本来就对。这里只验
//! 「fork 的 Co 变体被创建 → 读写经过钩子」，这是这次改动的全部内容。
//! 服务端接受连接后立即关闭，客户端会在读第一包时拿到 EOF——
//! 但读写调用已经发生过，计数就是证据。

#![cfg(feature = "mysql")]

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};

static HOOK_READS: AtomicUsize = AtomicUsize::new(0);

fn hook_read(s: &TcpStream, buf: &mut [u8]) -> io::Result<usize> {
    HOOK_READS.fetch_add(1, Ordering::Relaxed);
    (&*s).read(buf)
}

fn hook_write(s: &TcpStream, buf: &[u8]) -> io::Result<usize> {
    (&*s).write(buf)
}

/// 起一个"接受就关"的假服务端，返回地址。它只负责让 connect 成功、
/// 让客户端至少读到一次 EOF。
fn fake_server() -> std::net::SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || {
        for s in l.incoming() {
            drop(s);
        }
    });
    addr
}

/// 两条路径在**同一个测试**里顺序验：计数是进程级的，
/// 拆成两个 `#[test]` 会被并行执行互相污染（试过，会假失败）。
#[test]
fn co_scheme_uses_the_fork_socket_but_plain_does_not() {
    rsbatis_driver::set_mysql_socket_hooks(hook_read, hook_write);
    let addr = fake_server();

    let before = mysql::co_stats();
    // 握手必然失败（对端直接关），这不重要：要看的是"有没有走 Co"
    let _ = rsbatis_driver::mysql_driver::MySqlDriver::connect(&format!(
        "mysql+co://root:pw@{addr}/db"
    ));
    let after_co = mysql::co_stats();
    assert_eq!(
        after_co.0,
        before.0 + 1,
        "`mysql+co://` 没有创建 Co socket：{before:?} → {after_co:?}"
    );
    assert!(
        after_co.1 > before.1,
        "Co socket 建了但没走钩子读：{before:?} → {after_co:?}"
    );
    assert!(
        HOOK_READS.load(Ordering::Relaxed) > 0,
        "钩子一次都没被调到"
    );

    let _ = rsbatis_driver::mysql_driver::MySqlDriver::connect(&format!(
        "mysql://root:pw@{addr}/db"
    ));
    let after_plain = mysql::co_stats();
    assert_eq!(
        after_plain.0, after_co.0,
        "`mysql://` 不该走 Co：{after_co:?} → {after_plain:?}"
    );
}
