//! A streamed response body that ends cleanly only when its writer says so.
//!
//! If the [`Outlet`] is dropped without [`Outlet::finish`], the [`Inlet`] yields an error,
//! which makes hyper abort the connection: curl and wget then exit non-zero instead of
//! treating a broken Transfer as a complete (chunked) download. The error is held back
//! briefly so hyper first flushes what was already written (e.g. the "# failed" line).

use std::{
    future::Future,
    io,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};

use bytes::Bytes;
use hyper::body::{Body, Frame};
use tokio::{sync::mpsc, time::Sleep};

const ABORT_DELAY: Duration = Duration::from_millis(100);

pub fn channel(capacity: usize) -> (Outlet, Inlet) {
    let (tx, rx) = mpsc::channel(capacity);
    let done = Arc::new(AtomicBool::new(false));
    (
        Outlet {
            tx,
            done: done.clone(),
        },
        Inlet {
            rx,
            done,
            abort: None,
        },
    )
}

pub struct Outlet {
    tx: mpsc::Sender<Bytes>,
    done: Arc<AtomicBool>,
}

impl Outlet {
    /// `false` once the reading side has gone away.
    pub async fn send(&self, data: Bytes) -> bool {
        self.tx.send(data).await.is_ok()
    }

    pub async fn line(&self, text: impl Into<String>) -> bool {
        self.send(Bytes::from(text.into() + "\n")).await
    }

    /// Ends the body cleanly. Dropping the outlet without this aborts it.
    pub fn finish(self) {
        self.done.store(true, Ordering::Release);
    }

    /// Resolves when the reading side has gone away.
    pub async fn closed(&self) {
        self.tx.closed().await
    }
}

pub struct Inlet {
    rx: mpsc::Receiver<Bytes>,
    done: Arc<AtomicBool>,
    abort: Option<Pin<Box<Sleep>>>,
}

impl Body for Inlet {
    type Data = Bytes;
    type Error = io::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, io::Error>>> {
        match self.rx.poll_recv(cx) {
            Poll::Ready(Some(data)) => Poll::Ready(Some(Ok(Frame::data(data)))),
            Poll::Ready(None) if self.done.load(Ordering::Acquire) => Poll::Ready(None),
            Poll::Ready(None) => {
                let abort = self
                    .abort
                    .get_or_insert_with(|| Box::pin(tokio::time::sleep(ABORT_DELAY)));
                abort
                    .as_mut()
                    .poll(cx)
                    .map(|()| Some(Err(io::Error::other("transfer aborted"))))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use http_body_util::BodyExt;

    use super::*;

    #[tokio::test]
    async fn finished_body_ends_cleanly() {
        let (out, inlet) = channel(4);
        assert!(out.line("hi").await);
        out.finish();
        let collected = inlet.collect().await.unwrap().to_bytes();
        assert_eq!(collected, "hi\n");
    }

    #[tokio::test]
    async fn dropped_body_errors() {
        let (out, inlet) = channel(4);
        assert!(out.line("hi").await);
        drop(out);
        assert!(inlet.collect().await.is_err());
    }
}
