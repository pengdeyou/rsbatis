//! 64 位时间有序 id（Snowflake 布局）。
//!
//! 数据库自增主键有两个绕不过去的问题：一是必须先插入才知道 id（于是要么多一次
//! `RETURNING` 往返，要么字段写成 `Option<i64>`）；二是分库分表之后各表各自自增，
//! 全局不唯一。雪花 id 把"唯一性"挪到应用侧，主键在 `INSERT` 之前就已经是确定值。
//!
//! ```text
//! 63      62                22        12         0
//! +--------+-----------------+---------+----------+
//! | 符号 0 | 毫秒时间戳 41位  | 节点 10 | 序列 12  |
//! +--------+-----------------+---------+----------+
//! ```
//!
//! 41 位毫秒从 1970 起可撑到 2109 年；10 位节点 = 1024 个实例；12 位序列 =
//! 单节点每毫秒 4096 个，即单节点上限约 409.6 万/秒。
//!
//! # 三个必须知道的点
//!
//! 1. **节点 id 必须自己保证唯一**。两台机器都拿到 3 号，同一毫秒就会撞 id。
//!    这里默认从 `RSBATIS_NODE_ID` 环境变量取，取不到就按 `hostname + pid` 哈希——
//!    哈希能避免"手写死的默认值让所有实例都变 0 号"，但哈希**不是**分配机制，
//!    生产环境要显式配。
//! 2. **时钟回拨**。机器 NTP 校正往回跳时，如果毫秒数退回去，就可能生成重复 id。
//!    这里的策略是小幅回拨（≤5ms）原地等到上次发号的毫秒再继续，大幅回拨直接报错
//!    而不是"猜一个"——宁可插入失败，也不要写进一条和别的行撞主键的数据。
//! 3. **id > 2^53 之后 JSON 会失真**。雪花 id 是 19 位十进制，超过 JS 的
//!    `Number.MAX_SAFE_INTEGER`。给前端时必须序列化成字符串，这是所有雪花方案的
//!    共同税，不是实现问题。
//!
//! # 无锁
//!
//! 状态全打包在一个 `u64` 里（`毫秒 | 节点 | 序列`），用一条 CAS 原子更新，
//! 没有 `Mutex`、没有跨线程排队。争用失败就重读重试，最坏情况是同一毫秒内
//! 多个线程轮流拿到相邻序列号。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

const SEQ_BITS: u64 = 12;
const NODE_BITS: u64 = 10;
const NODE_MASK: u64 = (1 << NODE_BITS) - 1;
const SEQ_MASK: u64 = (1 << SEQ_BITS) - 1;
const NODE_SHIFT: u64 = SEQ_BITS;
const MS_SHIFT: u64 = SEQ_BITS + NODE_BITS;

/// 允许的时钟回拨幅度。超过这个值就报错，不再"借用未来毫秒"。
const MAX_BACKWARD_MS: i64 = 5;

static GLOBAL: OnceLock<Snowflake> = OnceLock::new();

/// id 生成失败的原因。目前只有一种：时钟回拨超出容忍范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdError {
    /// 时钟回拨了 `backward_ms` 毫秒，超过 `MAX_BACKWARD_MS`。
    ///
    /// 继续发号会让新 id 落进已发过的毫秒区间，可能重号。处理方式是等时钟追上来
    /// （或换一台机器），不要重试到死。
    ClockMovedBackwards {
        /// 回拨的毫秒数。
        backward_ms: i64,
    },
    /// 节点 id 超出 10 位（0..=1023）。
    NodeOutOfRange {
        /// 传进来的节点 id。
        node: u16,
    },
}

impl std::fmt::Display for IdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IdError::ClockMovedBackwards { backward_ms } => write!(
                f,
                "时钟回拨 {backward_ms}ms（容忍上限 {MAX_BACKWARD_MS}ms），拒绝发号以免重号"
            ),
            IdError::NodeOutOfRange { node } => {
                write!(f, "节点 id {node} 超出 10 位范围（0..=1023）")
            }
        }
    }
}

impl std::error::Error for IdError {}

/// 一个发号器实例。`Clone` 不共享状态，要共享就 `&'static`（见 [`Snowflake::global`]）。
#[derive(Debug)]
pub struct Snowflake {
    node: u64,
    state: AtomicU64,
}

impl Snowflake {
    /// 用指定节点 id 构造。节点 id 取 0..=1023。
    pub fn new(node: u16) -> Result<Self, IdError> {
        if u64::from(node) > NODE_MASK {
            return Err(IdError::NodeOutOfRange { node });
        }
        // 初值 0 表示"还没发过号"，第一次 next() 会走进 `ms > last_ms` 分支。
        Ok(Self {
            node: u64::from(node),
            state: AtomicU64::new(0),
        })
    }

    /// 进程级共享实例，节点 id 取自 `RSBATIS_NODE_ID`，取不到就按 `hostname + pid` 哈希。
    ///
    /// 哈希只解决"所有实例默认都是 0 号"这个最糟的默认值，**不保证**唯一。
    /// 多实例部署请显式设置 `RSBATIS_NODE_ID`。
    pub fn global() -> &'static Snowflake {
        GLOBAL.get_or_init(|| {
            let node = std::env::var("RSBATIS_NODE_ID")
                .ok()
                .and_then(|v| v.trim().parse::<u16>().ok())
                .filter(|n| u64::from(*n) <= NODE_MASK)
                .unwrap_or_else(hash_node);
            Snowflake {
                node: u64::from(node),
                state: AtomicU64::new(0),
            }
        })
    }

    /// 节点 id。
    pub fn node(&self) -> u16 {
        self.node as u16
    }

    /// 取下一个 id。
    pub fn next(&self) -> Result<i64, IdError> {
        loop {
            let last = self.state.load(Ordering::Relaxed);
            let last_ms = (last >> MS_SHIFT) as i64;
            let last_seq = last & SEQ_MASK;

            let mut now_ms = unix_ms();
            let mut seq;

            if now_ms < last_ms {
                let backward = last_ms - now_ms;
                if backward > MAX_BACKWARD_MS {
                    return Err(IdError::ClockMovedBackwards {
                        backward_ms: backward,
                    });
                }
                // 小幅回拨：不动毫秒，沿用一个序列号，等价于"把这几毫秒借过去"。
                now_ms = last_ms;
                seq = last_seq + 1;
                if seq > SEQ_MASK {
                    // 借的毫秒也被塞满了，只能等到时钟真的越过它。
                    while unix_ms() <= last_ms {
                        std::hint::spin_loop();
                    }
                    continue;
                }
            } else if now_ms == last_ms {
                seq = last_seq + 1;
                if seq > SEQ_MASK {
                    // 同一毫秒 4096 个已发完：自旋到下一毫秒。4096 个/ms 是上限，
                    // 正常情况下到不了这里。
                    while unix_ms() <= last_ms {
                        std::hint::spin_loop();
                    }
                    continue;
                }
            } else {
                seq = 0;
            }

            debug_assert!(seq <= SEQ_MASK);
            let new = ((now_ms as u64) << MS_SHIFT) | (self.node << NODE_SHIFT) | seq;

            if self
                .state
                .compare_exchange_weak(last, new, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                return Ok(new as i64);
            }
            // 被别人抢先了：重读，接着抢下一号。
        }
    }

    /// 拆解 id，返回 `(毫秒时间戳, 节点 id, 序列号)`。主要给排查和测试用。
    pub fn parse(id: i64) -> (i64, u16, u16) {
        let v = id as u64;
        (
            (v >> MS_SHIFT) as i64,
            ((v >> NODE_SHIFT) & NODE_MASK) as u16,
            (v & SEQ_MASK) as u16,
        )
    }
}

fn unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn hash_node() -> u16 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut h = DefaultHasher::new();
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_default()
        .hash(&mut h);
    std::process::id().hash(&mut h);
    (h.finish() & NODE_MASK) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_strictly_increasing() {
        let sf = Snowflake::new(7).unwrap();
        let mut prev = 0;
        for _ in 0..10_000 {
            let id = sf.next().unwrap();
            assert!(id > prev, "id 必须严格递增：{prev} -> {id}");
            prev = id;
        }
    }

    #[test]
    fn ids_are_unique_under_contention() {
        use std::sync::Arc;

        static SHARED: OnceLock<Arc<Snowflake>> = OnceLock::new();
        let sf = SHARED.get_or_init(|| Arc::new(Snowflake::new(1023).unwrap()));

        let mut handles = Vec::new();
        for _ in 0..8 {
            let sf = Arc::clone(sf);
            handles.push(std::thread::spawn(move || {
                let mut out = Vec::with_capacity(1000);
                for _ in 0..1000 {
                    out.push(sf.next().unwrap());
                }
                out
            }));
        }

        let mut all = Vec::new();
        for h in handles {
            all.extend(h.join().unwrap());
        }
        let total = all.len();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), total, "并发下出现重复 id");
        assert!(all.iter().all(|&id| id > 0));
    }

    #[test]
    fn parse_round_trips() {
        let sf = Snowflake::new(513).unwrap();
        let id = sf.next().unwrap();
        let (ms, node, seq) = Snowflake::parse(id);
        assert_eq!(node, 513);
        assert!(seq <= SEQ_MASK as u16);
        assert!((ms - unix_ms()).abs() < 5_000);
    }

    #[test]
    fn node_out_of_range_is_rejected() {
        assert_eq!(
            Snowflake::new(1024).unwrap_err(),
            IdError::NodeOutOfRange { node: 1024 }
        );
        assert!(Snowflake::new(1023).is_ok());
    }

    #[test]
    fn big_backward_clock_jump_errors_instead_of_duplicating() {
        let sf = Snowflake::new(0).unwrap();
        // 伪造一个"未来"的状态，模拟时钟往回跳了很久。
        let fake_future_ms = unix_ms() + 1_000_000;
        sf.state.store(
            (fake_future_ms as u64) << MS_SHIFT,
            Ordering::Relaxed,
        );
        match sf.next() {
            Err(IdError::ClockMovedBackwards { backward_ms }) => {
                assert!(backward_ms > MAX_BACKWARD_MS);
            }
            other => panic!("应报时钟回拨，实际 {other:?}"),
        }
    }
}
