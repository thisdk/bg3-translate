//! 事件出口与取消令牌：引擎与外部世界（Tauri Channel / 单测收集器）之间
//! 唯一的接触面。
//!
//! 这两样东西都刻意做得极简：
//! - [`EventSink::emit`] 不返回错误（底层通道失败只记日志，前端断开不该让翻译崩掉）；
//! - [`CancelToken`] 就是一个 `Arc<AtomicBool>`，取消靠轮询生效，
//!   代价是取消最多延迟 [`CANCEL_POLL_INTERVAL`]（与旧实现一致）。

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::types::TranslationEvent;

/// 取消轮询粒度：与旧实现一致，最多 100ms 延迟生效。
pub(crate) const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// 事件出口：真实运行时是 Tauri Channel，单测里是 [`CollectingSink`]。
///
/// `emit` 不返回错误：底层通道发送失败只记日志（旧实现的 Tauri Channel
/// 失败也是被忽略的），翻译流程不应该因为前端断开就崩掉。
pub trait EventSink: Send + Sync + 'static {
    fn emit(&self, event: TranslationEvent);
}

/// 收集事件的内存 sink，供单测断言事件序列。
#[derive(Debug, Default)]
pub struct CollectingSink {
    events: Mutex<Vec<TranslationEvent>>,
}

impl CollectingSink {
    pub fn new() -> Self {
        Self::default()
    }

    /// 复制一份已收集的事件。
    pub fn events(&self) -> Vec<TranslationEvent> {
        self.lock().clone()
    }

    /// 取出全部事件（消费 sink）。
    pub fn into_events(self) -> Vec<TranslationEvent> {
        self.events
            .into_inner()
            .unwrap_or_else(|err| err.into_inner())
    }

    /// 中毒的锁也要能用：断言失败导致 panic 后仍希望看到已收集的事件。
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<TranslationEvent>> {
        self.events.lock().unwrap_or_else(|err| err.into_inner())
    }
}

impl EventSink for CollectingSink {
    fn emit(&self, event: TranslationEvent) {
        self.lock().push(event);
    }
}

/// 可克隆的取消令牌（所有克隆共享同一个标志）。
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// 请求取消；已在飞行中的请求会尽快返回 `Ok(None)`。
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    /// 清除取消标志，供下一次翻译复用同一个令牌。
    pub fn reset(&self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// 轮询等待取消。
///
/// 用轮询而不是通知，是为了让 `CancelToken` 保持"一个 AtomicBool"这么简单；
/// 代价是取消最多延迟 [`CANCEL_POLL_INTERVAL`] 生效（与旧实现一致）。
pub(crate) async fn wait_until_cancelled(cancel: CancelToken) {
    while !cancel.is_cancelled() {
        tokio::time::sleep(CANCEL_POLL_INTERVAL).await;
    }
}

/// 发「开始翻译」。
pub(crate) fn emit_progress(sink: &dyn EventSink, entry_id: &str) {
    sink.emit(TranslationEvent::progress(entry_id));
}

/// 发「开始翻译 + 完成」（规划阶段的直接完成走这里，与旧 `send_done` 一致）。
pub(crate) fn emit_done(sink: &dyn EventSink, entry_id: &str, text: String) {
    emit_progress(sink, entry_id);
    sink.emit(TranslationEvent::done(entry_id, text));
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::types::TranslationStatus;

    #[test]
    fn cancel_token_is_shared_between_clones() {
        let token = CancelToken::new();
        let clone = token.clone();
        assert!(!token.is_cancelled());
        clone.cancel();
        assert!(token.is_cancelled(), "克隆共享同一个标志");
        token.reset();
        assert!(!clone.is_cancelled());
        assert!(!CancelToken::default().is_cancelled());
    }

    #[test]
    fn collecting_sink_records_events_in_order() {
        let sink = CollectingSink::new();
        sink.emit(TranslationEvent::progress("a"));
        sink.emit(TranslationEvent::done("a", "甲"));
        assert_eq!(
            sink.events(),
            vec![
                TranslationEvent::progress("a"),
                TranslationEvent::done("a", "甲")
            ]
        );
        assert_eq!(
            sink.into_events(),
            vec![
                TranslationEvent::progress("a"),
                TranslationEvent::done("a", "甲")
            ]
        );
    }

    #[test]
    fn progress_events_carry_translating_status() {
        let sink = CollectingSink::new();
        emit_done(&sink, "e1", "译文".into());
        assert_eq!(
            sink.events(),
            vec![
                TranslationEvent::Progress {
                    entry_id: "e1".into(),
                    status: TranslationStatus::Translating,
                },
                TranslationEvent::done("e1", "译文"),
            ]
        );
    }

    /// 壳层（`src-tauri::ChannelSink`）在自己的 crate 里实现 [`EventSink`]。
    /// `src-tauri` 依赖 webkit2gtk 等 GUI 系统库，本机与 ubuntu CI 都编译不了，
    /// 所以用同样的形态在这里守住 trait 的形状：`emit(&self, event)` 不返回
    /// `Result`，实现类型必须 `Send + Sync + 'static`（引擎用 `&dyn EventSink`）。
    #[test]
    fn event_sink_can_be_implemented_like_the_tauri_shell_does() {
        struct ChannelLikeSink {
            send: Box<dyn Fn(TranslationEvent) + Send + Sync>,
        }

        impl EventSink for ChannelLikeSink {
            fn emit(&self, event: TranslationEvent) {
                (self.send)(event);
            }
        }

        let received: Arc<Mutex<Vec<TranslationEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = ChannelLikeSink {
            send: Box::new({
                let received = Arc::clone(&received);
                move |event| received.lock().unwrap().push(event)
            }),
        };

        emit_progress(&sink, "e1");
        sink.emit(TranslationEvent::done("e1", "译文"));

        let shared: Arc<dyn EventSink> = Arc::new(sink);
        shared.emit(TranslationEvent::progress("e2"));
        let cloned = Arc::clone(&shared);
        std::thread::spawn(move || cloned.emit(TranslationEvent::progress("e3")))
            .join()
            .unwrap();

        let events = received.lock().unwrap().clone();
        assert_eq!(
            events,
            vec![
                TranslationEvent::progress("e1"),
                TranslationEvent::done("e1", "译文"),
                TranslationEvent::progress("e2"),
                TranslationEvent::progress("e3"),
            ]
        );
    }
}
