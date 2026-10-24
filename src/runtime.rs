//! Bridges GTK's main loop and tokio: network futures run on a shared tokio
//! runtime, UI code awaits their results from `glib::spawn_future_local`.

use std::future::Future;
use std::sync::OnceLock;

use tokio::runtime::Runtime;

fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to start the tokio runtime")
    })
}

/// Runs `future` on tokio; the returned future (awaitable on the GTK main
/// thread) resolves to its output.
pub fn spawn_tokio<F>(future: F) -> impl Future<Output = F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let result = spawn_tokio_checked(future);
    async move {
        result
            .await
            .expect("tokio task panicked before sending its result")
    }
}

/// Like [`spawn_tokio`], but a task that panicked (or was dropped) before
/// producing its output is an error the caller can show, instead of a panic
/// on the GTK main thread. For flows where the UI waits on the task.
pub fn spawn_tokio_checked<F>(future: F) -> impl Future<Output = anyhow::Result<F::Output>>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let (sender, receiver) = tokio::sync::oneshot::channel();
    runtime().spawn(async move {
        let _ = sender.send(future.await);
    });
    async move {
        receiver
            .await
            .map_err(|_| anyhow::anyhow!("the task failed unexpectedly, see the log"))
    }
}

/// Runs `future` on tokio and blocks the calling (non-tokio) thread until
/// it finishes or `timeout` passes. For shutdown only, where the UI is
/// going away anyway.
pub fn block_on_timeout<F>(future: F, timeout: std::time::Duration) -> Option<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    runtime()
        .block_on(async move { tokio::time::timeout(timeout, future).await })
        .ok()
}

/// Fire-and-forget on tokio, for requests nobody waits on (e.g. reporting).
pub fn spawn_detached<F>(future: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    runtime().spawn(future);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_tokio_returns_the_output() {
        let value = gtk::glib::MainContext::new().block_on(spawn_tokio(async { 40 + 2 }));
        assert_eq!(value, 42);
    }

    #[test]
    fn spawn_tokio_checked_reports_a_panicked_task() {
        let ctx = gtk::glib::MainContext::new();
        let failed = ctx.block_on(spawn_tokio_checked(async {
            if true {
                panic!("boom");
            }
            1
        }));
        assert!(failed.is_err());
        assert_eq!(ctx.block_on(spawn_tokio_checked(async { 7 })).unwrap(), 7);
    }
}
