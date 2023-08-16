use std::{
    pin::Pin,
    task::{Context, Poll},
};

use smol::{
    prelude::AsyncRead,
    stream::{Stream, StreamExt},
};

use super::tokenizer::{Interner, Tokenizer};
use crate::{
    dom::{Dom, EMPTY_RANGE_INDEX},
    io::{Location, PeekableStream, PeekableStreamable},
};

#[derive(thiserror::Error, Debug, Clone)]
#[error("{msg}")]
pub struct ParserError {
    loc: Location,
    msg: String,
}

#[derive(Copy, Clone)]
enum InsertionMode {
    Initial,
    BeforeHtml,
    BeforeHead,
    InHead,
    AfterHead,
    InBody,
    AfterBody,
    AfterAfterBody,
}

#[pin_project::pin_project(project = ProjectedParser)]
pub struct Parser<R: AsyncRead + Unpin> {
    #[pin]
    tokenizer: PeekableStream<Tokenizer<R, Dom>>,
    shoes: String,
}

impl Interner for Dom {
    const EMPTY_RANGE_INDEX: usize = EMPTY_RANGE_INDEX;
    fn intern_str(&mut self, s: &str) -> usize {
        self.insert_text(s)
    }
    fn intern_attrs(&mut self, attrs: &[[usize; 2]]) -> usize {
        self.insert_attrs(attrs)
    }
}

pub enum ParseEvent {
    Title, // TODO: fire off title when we have it
    Link,  // TODO: need to fire off when a link tag is ready to fetch
    Style, // TODO: style tag contents are ready to parse
}

impl<'a, R: AsyncRead + Unpin> ProjectedParser<'a, R> {
    fn parse_next(self, cx: &mut Context<'_>) -> Poll<Option<Result<ParseEvent, ParserError>>> {
        self.tokenizer.get_mut().poll_next(cx);
        Poll::Ready(None)
    }
}

impl<R: AsyncRead + Unpin> Stream for Parser<R> {
    type Item = Result<ParseEvent, ParserError>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.project().parse_next(cx)
    }
}
