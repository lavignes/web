use std::{
    pin::Pin,
    task::{Context, Poll},
};

use smol::{
    prelude::AsyncRead,
    stream::{Stream, StreamExt},
};

use super::tokenizer::{Interner, Token, Tokenizer, TokenizerError};
use crate::{
    dom::{Dom, EMPTY_RANGE_INDEX},
    io::{AsyncStrError, Location, PeekableStream, PeekableStreamable},
};

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

struct ParserInner {
    insertion_mode: InsertionMode,
    tok_buf: Vec<(Location, Token)>,
}

#[must_use]
#[pin_project::pin_project]
pub struct Parser<R: AsyncRead + Unpin> {
    #[pin]
    tokenizer: Tokenizer<R>,
    inner: ParserInner,
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
    Fatal(Location, ()),
    Title, // TODO: fire off title when we have it
    Link,  // TODO: need to fire off when a link tag is ready to fetch
    Style, // TODO: style tag contents are ready to parse
}

/*
impl<R: AsyncRead + Unpin> Parser<R> {
    fn peek(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Option<(Location, Token)>, (Location, TokenizerError)>> {
        let pin = Pin::new(&mut self.tokenizer);
        pin.poll_peek(cx).ready()?.map_or_else(
            || Poll::Ready(Ok(None)),
            |pair| match pair {
                (loc, Ok(tok)) => Poll::Ready(Ok(Some((loc, tok)))),
                (loc, Err(err)) => Poll::Ready(Err((loc, err))),
            },
        )
    }
}

impl ParserInner {
    fn next(&mut self, dom: &mut Dom) -> Poll<Option<ParseEvent>> {
        loop {
            match self.insertion_mode {}
        }
    }
}
*/

impl<R: AsyncRead + Unpin> Parser<R> {
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>, dom: &mut Dom) -> Poll<Option<ParseEvent>> {
        let this = self.project();

        this.inner.next(this.tokenizer)

        /*
        loop {
            match this.insertion_mode {
                InsertionMode::Initial => match peek!(this, cx)? {
                    Err((loc, _)) => return Poll::Ready(Some(ParseEvent::Fatal(loc, ()))),
                    Ok(Some((_, Token::Comment | Token::Char('\t' | '\n' | '\x0C' | ' ')))) => {
                        next!(this, cx)?;
                    }
                    Ok(Some((_, Token::DocType))) => todo!("doctype"),
                    _ => {
                        *this.insertion_mode = InsertionMode::BeforeHtml;
                    }
                },

                _ => todo!(),
            }
        }
        */
    }
}
*/
