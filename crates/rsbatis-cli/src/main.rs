//! `rsbatis` 可执行入口。逻辑全在 lib.rs（这样测试不用起进程）。

use std::io::Write;

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let env_url = std::env::var("DATABASE_URL").ok();
    let out = rsbatis_cli::run(&argv, env_url);

    if !out.stdout.is_empty() {
        print!("{}", out.stdout);
    }
    if !out.stderr.is_empty() {
        eprint!("{}", out.stderr);
    }
    // 显式刷新：管道被缓冲时，进程退出前没 flush 会丢最后一段输出
    let _ = std::io::stdout().flush();
    std::process::exit(out.code);
}
