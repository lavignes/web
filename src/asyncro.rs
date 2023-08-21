use std::{
    pin::Pin,
    ptr,
    task::{Context, Poll, RawWaker, RawWakerVTable, Waker},
};

use smol::stream::Stream;

/// Homespun version of `futures-rs`'s `future::task::noop_waker_ref`
pub fn noop_waker_ref() -> &'static Waker {
    struct SyncRawWaker(RawWaker);
    unsafe impl Sync for SyncRawWaker {}
    const RAW: RawWaker = RawWaker::new(ptr::null(), &VTABLE);
    const VTABLE: RawWakerVTable = RawWakerVTable::new(|_| RAW, |_| {}, |_| {}, |_| {});
    static NOOP_WAKER_INSTANCE: SyncRawWaker = SyncRawWaker(RAW);
    // Safety: `Waker` is #[repr(transparent)] over its `RawWaker`.
    unsafe { &*(&NOOP_WAKER_INSTANCE.0 as *const RawWaker as *const Waker) }
}

// TODO: ununsed?
#[must_use]
#[pin_project::pin_project]
pub struct PeekableStream<S: Stream> {
    #[pin]
    inner: S,
    peeked: Option<Poll<Option<S::Item>>>,
}

impl<S: Stream> PeekableStream<S> {
    pub fn poll_peek(self: Pin<&mut Self>, cx: &mut Context<'_>) -> &Poll<Option<S::Item>> {
        let this = self.project();
        this.peeked.get_or_insert_with(|| this.inner.poll_next(cx))
    }
}

pub trait PeekableStreamable {
    fn peekable(self) -> PeekableStream<Self>
    where
        Self: Sized + Stream;
}

impl<S: Stream> PeekableStreamable for S {
    fn peekable(self) -> PeekableStream<Self> {
        PeekableStream {
            inner: self,
            peeked: None,
        }
    }
}

impl<S: Stream> Stream for PeekableStream<S> {
    type Item = S::Item;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();
        match this.peeked.take() {
            Some(item) => item,
            None => this.inner.poll_next(cx),
        }
    }
}
