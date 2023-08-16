use std::{
    ptr,
    task::{RawWaker, RawWakerVTable, Waker},
};

pub use asyncstr::*;
pub use newline_normalize::*;

mod asyncstr;
mod newline_normalize;

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Location {
    pub line: usize,
    pub column: usize,
}

impl From<[usize; 2]> for Location {
    fn from(value: [usize; 2]) -> Self {
        Self {
            line: value[0],
            column: value[1],
        }
    }
}

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
