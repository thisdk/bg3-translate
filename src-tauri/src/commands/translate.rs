//! 流式翻译命令。

use std::sync::atomic::{AtomicBool, Ordering};

use bg3_translate_core::config;
use bg3_translate_core::glossary::Glossary;
use bg3_translate_core::translation::{EventSink, RunOptions, TranslationEngine};
use bg3_translate_core::types::{LlmSettings, TranslationEntry, TranslationEvent};
use bg3_translate_core::{AppError, Result};
use tauri::State;
use tauri::ipc::Channel;

use crate::state::AppState;

/// 把 core 的事件桥接到 Tauri Channel。
///
/// 推送失败本身不该让翻译任务报错：前端窗口关掉之后 `send` 就会失败，而译文
/// 仍然在引擎的返回值里。但**第一次**失败必须留下痕迹 —— release 构建的日志
/// 级别是 Info，早先统一按 debug 记的写法会被过滤掉，于是一条已经断掉的通道
/// 就是「翻译照跑、结果没人收」：用户白白花 API 额度，日志里一个字都没有。
/// 第 2 次起降回 debug，避免一条死通道把日志刷满。
struct ChannelSink {
    channel: Channel<TranslationEvent>,
    /// 是否已经记录过一次推送失败（每个任务只报一次）
    push_failed: AtomicBool,
}

impl ChannelSink {
    fn new(channel: Channel<TranslationEvent>) -> Self {
        Self {
            channel,
            push_failed: AtomicBool::new(false),
        }
    }

    /// 记下一次推送失败，返回这次该用的日志级别。
    ///
    /// 抽成函数是为了能被单测直接钉住：`log` 的全局 logger 在测试进程里只能装
    /// 一次（同模块的日志用例已经占用了），所以「第一次用 warn、之后降回 debug」
    /// 这条决策必须在不依赖 logger 的前提下可断言。
    fn note_push_failure(&self) -> log::Level {
        if self.push_failed.swap(true, Ordering::Relaxed) {
            log::Level::Debug
        } else {
            log::Level::Warn
        }
    }
}

impl EventSink for ChannelSink {
    fn emit(&self, event: TranslationEvent) {
        if let Err(err) = self.channel.send(event) {
            match self.note_push_failure() {
                log::Level::Warn => {
                    log::warn!(
                        "翻译事件推送失败（前端可能已关闭），本轮后续同类失败只记 debug: {err}"
                    );
                }
                _ => log::debug!("翻译事件推送失败（前端可能已关闭）: {err}"),
            }
        }
    }
}

/// 流式翻译条目。
///
/// - 只翻译 `source` 非空且 `target` 为空的条目（其余直接跳过）
/// - 事件通过 `onEvent` Channel 实时推送
/// - 期间可以用 `cancel_translation` 取消
#[tauri::command]
pub async fn translate_entries(
    state: State<'_, AppState>,
    work_dir: String,
    entries: Vec<TranslationEntry>,
    style_hint: Option<String>,
    on_event: Channel<TranslationEvent>,
) -> Result<()> {
    // 工作目录目前只在日志里用：条目自带 source_file，翻译不需要读盘。
    // 保留这个参数是为了跟前端的既有调用保持一致（Tauri 的形参名必须能
    // 从 JS 的 camelCase 反查回来，所以不能写成 `_work_dir`）。
    log_translate_request(&work_dir, entries.len());

    let settings = resolve_settings(&state).await?;
    if !settings.is_configured() {
        return Err(AppError::config(
            "未配置 API Key，请先在设置中填写大模型连接信息",
        ));
    }

    let glossary = tokio::task::spawn_blocking(Glossary::load)
        .await
        .map_err(|err| AppError::other(format!("读取术语表任务异常终止: {err}")))??;
    let matcher = glossary.matcher();

    state.cancel.reset();
    let sink = ChannelSink::new(on_event);

    let engine = TranslationEngine::with_settings(settings)?;
    let summary = engine
        .run(
            &entries,
            &matcher,
            RunOptions {
                style_hint: style_hint.as_deref().unwrap_or_default(),
                sink: &sink,
                cancel: state.cancel.clone(),
            },
        )
        .await?;

    // 汇总结果这里只记日志：前端已经通过 all_done 事件拿到 total/failed，
    // 命令返回值保持 void，避免契约里多一个需要两边同步的结构。
    log::info!(
        "翻译结束：待翻译 {} 条，成功 {} 条，失败 {} 条，取消={}",
        summary.total,
        summary.translated,
        summary.failed,
        summary.cancelled
    );
    Ok(())
}

/// 记录一条「翻译请求」日志。
///
/// 抽成函数是为了能被单测直接驱动：`work_dir` 是**前端可控字符串**，
/// 日志同时写控制台与日志文件，排查问题时会被当成证据 —— 一个
/// `work_dir = "…\n[INFO] 翻译结束：成功 999 条"` 就能在日志里插进一条
/// 不存在的记录。所以这里必须用 Debug（`{:?}`）转义换行与不可见字符。
fn log_translate_request(work_dir: &str, entry_count: usize) {
    log::debug!("翻译请求：work_dir={work_dir:?}，条目 {entry_count} 条");
}

/// 请求取消当前翻译任务。
#[tauri::command]
pub async fn cancel_translation(state: State<'_, AppState>) -> Result<()> {
    log::info!("收到取消翻译请求");
    state.cancel.cancel();
    Ok(())
}

/// 设置来源：内存缓存优先，其次磁盘，最后默认值。
async fn resolve_settings(state: &State<'_, AppState>) -> Result<LlmSettings> {
    if let Some(cached) = state.settings_guard().map_err(AppError::config)?.clone() {
        return Ok(cached);
    }
    let loaded = tokio::task::spawn_blocking(config::load)
        .await
        .map_err(|err| AppError::other(format!("读取设置任务异常终止: {err}")))??;
    *state.settings_guard().map_err(AppError::config)? = Some(loaded.clone());
    Ok(loaded)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    /// 捕获日志记录的测试 logger。
    ///
    /// 只服务于「前端可控字符串不得伪造日志行」这条防线：日志是排查问题的
    /// 依据，而 `work_dir` 完全由前端控制，所以这条防线必须能被自动验证。
    struct CaptureLogger(Arc<Mutex<Vec<String>>>);

    impl log::Log for CaptureLogger {
        fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
            true
        }

        fn log(&self, record: &log::Record<'_>) {
            if let Ok(mut records) = self.0.lock() {
                records.push(record.args().to_string());
            }
        }

        fn flush(&self) {}
    }

    /// 通道断掉（前端窗口已关闭 / 开发时热重载）时，**第一次**推送失败必须用
    /// `warn` 级别记下来。
    ///
    /// 旧写法每次失败都按 debug 记，而 release 构建的日志级别是 Info
    /// （见 `lib.rs::log_plugin`）：通道一断，翻译继续跑、事件全丢，日志里却
    /// 什么都没有 —— 用户白白花掉 API 额度。这里钉住「第一次 warn、之后 debug」
    /// 的决策；第 2 次起降级是为了不让一条死通道把日志刷满。
    #[test]
    fn channel_sink_reports_first_push_failure_at_warn_level() {
        let channel = Channel::new(|_| Err(tauri::Error::AssetNotFound("前端已关闭".into())));
        let sink = ChannelSink::new(channel);

        assert_eq!(
            sink.note_push_failure(),
            log::Level::Warn,
            "第一次推送失败必须用 warn（release 只打 Info 及以上，debug 会被静默过滤）"
        );
        assert_eq!(
            sink.note_push_failure(),
            log::Level::Debug,
            "后续失败降回 debug，避免一条死通道刷满日志"
        );

        // 真实推送路径：通道失败被吞掉（不影响翻译任务本身），状态照样记录
        sink.emit(TranslationEvent::AllDone {
            total: 1,
            failed: 0,
        });
        assert_eq!(sink.note_push_failure(), log::Level::Debug);
    }

    /// `work_dir` 里的换行必须在日志里被转义成可见形式，否则前端可以
    /// 用 `"…\n[INFO] 翻译结束：成功 999 条"` 在日志里伪造一整行记录。
    ///
    /// 断言只针对「含 `work_dir=` 的那条记录」，这样同进程里并行跑的其它
    /// 用例打出来的日志不会让这条断言变脆。
    #[test]
    fn translate_request_log_cannot_be_forged_with_newlines() {
        let captured = Arc::new(Mutex::new(Vec::new()));
        // 测试二进制里只有这一个安装者；装不上说明有人抢先了，直接报出来。
        log::set_boxed_logger(Box::new(CaptureLogger(captured.clone())))
            .expect("测试 logger 必须能装上");
        log::set_max_level(log::LevelFilter::Trace);

        let forged =
            "C:\\mods\\x\n[INFO] 翻译结束：待翻译 0 条，成功 999 条，失败 0 条，取消=false";
        log_translate_request(forged, 3);

        let records: Vec<String> = captured
            .lock()
            .expect("日志缓冲锁")
            .iter()
            .filter(|record| record.contains("work_dir="))
            .cloned()
            .collect();
        assert_eq!(records.len(), 1, "应恰好记录一条翻译请求日志: {records:?}");
        let record = &records[0];
        assert!(record.contains("条目 3 条"), "条目数应照常记录: {record}");
        assert!(
            !record.contains('\n') && !record.contains('\r'),
            "前端可控字符串不得在日志里引入换行（可伪造日志行）: {record:?}"
        );
        assert!(
            record.contains("\\n"),
            "换行应被转义成可见的 `\\n`: {record:?}"
        );
    }
}
