//! 只读不画的 ConPTY 吞吐基线。
//!
//! 目的:把"瓶颈在渲染之前"从推断变成实测。
//! 用和 VectorTerm 完全相同的 `portable-pty` 参数开一个 ConPTY,让子进程
//! `cmd /c type <payload>` 往里灌 8 MiB,本程序只做一件事 —— 全速读空管道,
//! 不解析、不渲染、不上屏。
//!
//! 读到的吞吐如果和上屏时的端到端吞吐是同一个量级,说明终端软件层面
//! (解析+排版+绘制)根本不在关键路径上,天花板在 ConPTY/conhost 这一层,
//! 换渲染后端、换解析器、换语言都不会有数量级收益。
//!
//!     cargo run --release --example conpty_ceiling -- target/bench_big_104000.txt

use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, CommandBuilder, PtySize};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/bench_big_104000.txt".to_string());
    let bytes_on_disk = std::fs::metadata(&path)?.len();
    let mib = bytes_on_disk as f64 / (1024.0 * 1024.0);

    let pty_system = native_pty_system();
    // 和 src/pty.rs 用一样的尺寸,免得 conhost 的换行/滚屏行为和终端里不一致
    let pair = pty_system.openpty(PtySize {
        rows: 40,
        cols: 120,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    let mut cmd = CommandBuilder::new("cmd.exe");
    // cmd 的 type 不认正斜杠路径,统一换成反斜杠
    let win_path = path.replace('/', "\\");
    cmd.args(["/c", "type", &win_path]);

    let start = Instant::now();
    let mut child = pair.slave.spawn_command(cmd)?;
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader()?;
    let seen = Arc::new(AtomicU64::new(0));
    // 首字节 / 末字节的微秒时间戳,用来把 conhost 的启动握手从流式时间里剥出来
    let first_us = Arc::new(AtomicU64::new(0));
    let last_us = Arc::new(AtomicU64::new(0));
    let counter = Arc::clone(&seen);
    let f = Arc::clone(&first_us);
    let l = Arc::clone(&last_us);
    let pump = std::thread::spawn(move || {
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let now = start.elapsed().as_micros() as u64;
                    if f.load(Ordering::Relaxed) == 0 {
                        f.store(now, Ordering::Relaxed);
                    }
                    l.store(now, Ordering::Relaxed);
                    counter.fetch_add(n as u64, Ordering::Relaxed);
                }
            }
        }
    });

    // ConPTY 的输出管道在伪控制台关闭前不会 EOF,所以用"子进程退出"当结束信号
    let _ = child.wait();
    let t_child = start.elapsed();

    // 等最后一批字节到齐:ConPTY 会把 LF 改写成 CRLF,通道字节数 >= 文件字节数
    let deadline = Instant::now() + Duration::from_secs(10);
    while seen.load(Ordering::Relaxed) < bytes_on_disk && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    let elapsed = start.elapsed();

    let total = seen.load(Ordering::Relaxed);
    drop(pair.master);
    let _ = pump.join();

    let secs = elapsed.as_secs_f64();
    println!("文件        : {path}");
    println!("磁盘字节    : {bytes_on_disk} ({mib:.2} MiB)");
    println!(
        "管道读到    : {total} ({:.2} MiB)",
        total as f64 / 1048576.0
    );
    println!("耗时        : {:.2} s", secs);
    println!("  其中子进程退出前 : {:.2} s", t_child.as_secs_f64());
    let t0 = first_us.load(Ordering::Relaxed) as f64 / 1e6;
    let t1 = last_us.load(Ordering::Relaxed) as f64 / 1e6;
    println!("  首字节 {:.2} s -> 末字节 {:.2} s", t0, t1);
    let stream = (t1 - t0).max(1e-3);
    println!(
        "流式区间    : {:.2} s  ->  {:.2} MiB/s",
        stream,
        mib / stream
    );
    println!("纯读取吞吐  : {:.2} MiB/s", mib / secs);
    println!(
        "折算到通道  : {:.2} MiB/s",
        total as f64 / 1048576.0 / secs
    );
    Ok(())
}
