use std::io::IsTerminal;

use tracing::Level;

/// 初始化日志：输出到 stderr，`-q` 时只保留错误。
///
/// 进度行不走日志（它是瞬时 UI），因此不会被这里过滤掉。
/// 非终端下关闭 ANSI，保证日志可以直接 grep。
pub fn init(quiet: bool) {
    let level = if quiet { Level::ERROR } else { Level::INFO };
    let _ = tracing_subscriber::fmt()
        .with_max_level(level)
        .with_target(false)
        .with_ansi(std::io::stderr().is_terminal())
        .with_writer(std::io::stderr)
        .try_init();
}
