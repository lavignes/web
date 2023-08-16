use std::{
    io,
    pin::Pin,
    str,
    task::{Context, Poll},
};

use smol::{io::AsyncRead, stream::Stream};

use crate::io::{AsyncStrError, AsyncStrReader, Location, NewlineNormalizable};

#[derive(thiserror::Error, Debug)]
pub enum TokenizerError {
    #[error(transparent)]
    IoError(#[from] io::Error),

    #[error(transparent)]
    Utf8Error(#[from] str::Utf8Error),

    #[error("HTML is malformed: {msg}")]
    Malformed { msg: String },
}

impl From<AsyncStrError> for TokenizerError {
    fn from(value: AsyncStrError) -> Self {
        match value {
            AsyncStrError::IoError(err) => Self::from(err),
            AsyncStrError::Utf8Error(err) => Self::from(err),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Token {
    Char(char),
    StartTag {
        name: usize,
        attrs: usize,
        self_closing: bool,
    },
    EndTag {
        name: usize,
        attrs: usize,
    },
    DocType,
    Comment,
}

enum State {
    Data,

    TagOpen,
    EndTagOpen,
    TagName,
    SelfClosingStartTag,
    BeforeAttributeName,
    AttributeName,
    AfterAttributeName,
    BeforeAttributeValue,
    AttributeValueDoubleQuote,
    AttributeValueSingleQuote,
    AttributeValueNoQuote,
    AfterAttributeValueQuoted,
}

pub trait Interner {
    const EMPTY_RANGE_INDEX: usize;
    fn intern_str(&mut self, s: &str) -> usize;
    fn intern_attrs(&mut self, attrs: &[[usize; 2]]) -> usize;
}

struct TokenizerInner<I> {
    consumed: usize,
    interner: I,
    state: State,
    str_buf: String,
    attr_buf: Vec<[usize; 2]>,
    tok: Token,
    start_loc: Location,
    loc: Location,
}

#[pin_project::pin_project]
pub struct Tokenizer<R, I> {
    #[pin]
    reader: AsyncStrReader<R>,
    inner: TokenizerInner<I>,
}

impl<R, I> Tokenizer<R, I> {
    pub fn new(reader: AsyncStrReader<R>, interner: I) -> Self {
        let inner = TokenizerInner {
            consumed: 0,
            interner,
            state: State::Data,
            str_buf: String::new(),
            attr_buf: Vec::new(),
            tok: Token::Comment,
            start_loc: Location { line: 1, column: 1 },
            loc: Location { line: 1, column: 0 },
        };
        Self { reader, inner }
    }
}

type TokenzizerItem = (Location, Result<Token, TokenizerError>);

impl<I: Interner> TokenizerInner<I> {
    fn malformed<M: ToString>(
        &mut self,
        loc: Location,
        msg: M,
    ) -> Poll<(usize, Option<TokenzizerItem>)> {
        Poll::Ready((
            self.consumed,
            Some((
                loc,
                Err(TokenizerError::Malformed {
                    msg: msg.to_string(),
                }),
            )),
        ))
    }

    fn token(&mut self, loc: Location, tok: Token) -> Poll<(usize, Option<TokenzizerItem>)> {
        Poll::Ready((self.consumed, Some((loc, Ok(tok)))))
    }

    fn malformed_here<M: ToString>(&mut self, msg: M) -> Poll<(usize, Option<TokenzizerItem>)> {
        self.malformed(self.loc, msg)
    }

    fn token_here(&mut self, tok: Token) -> Poll<(usize, Option<TokenzizerItem>)> {
        self.token(self.loc, tok)
    }

    fn set_tag_name_if_unset(&mut self) {
        self.tok = match self.tok {
            Token::StartTag {
                name,
                attrs,
                self_closing,
            } if name == I::EMPTY_RANGE_INDEX => {
                let name = self.interner.intern_str(&self.str_buf);
                Token::StartTag {
                    name,
                    attrs,
                    self_closing,
                }
            }
            Token::EndTag { name, attrs } if name == I::EMPTY_RANGE_INDEX => {
                let name = self.interner.intern_str(&self.str_buf);
                Token::EndTag { name, attrs }
            }
            tok => tok,
        }
    }

    fn set_tag_attrs_if_unset(&mut self) {
        self.tok = match self.tok {
            Token::StartTag {
                name,
                attrs,
                self_closing,
            } if attrs == I::EMPTY_RANGE_INDEX => {
                let attrs = self.interner.intern_attrs(&self.attr_buf);
                Token::StartTag {
                    name,
                    attrs,
                    self_closing,
                }
            }
            Token::EndTag { name, attrs } if attrs == I::EMPTY_RANGE_INDEX => {
                let attrs = self.interner.intern_attrs(&self.attr_buf);
                Token::EndTag { name, attrs }
            }
            tok => tok,
        }
    }

    fn consume<C: Iterator<Item = (usize, char)>>(&mut self, chars: &mut C) {
        match chars.next() {
            Some((len, '\n')) => {
                self.loc.line += 1;
                self.loc.column = 1;
                self.consumed += len;
            }
            Some((len, _)) => {
                self.loc.column += 1;
                self.consumed += len;
            }
            _ => {}
        }
    }

    fn next(&mut self, input: &str) -> Poll<(usize, Option<TokenzizerItem>)> {
        let mut chars = input.chars().newline_normalized().peekable();
        loop {
            let c = chars.peek().map(|(_, c)| *c);
            match self.state {
                State::Data => match c {
                    Some('&') => todo!("char reference state"),
                    Some('<') => {
                        self.consume(&mut chars);
                        self.state = State::TagOpen;
                        self.start_loc = self.loc;
                    }
                    Some('\x00') => return self.malformed_here("unexpected null byte"),
                    Some(c) => {
                        self.consume(&mut chars);
                        return self.token_here(Token::Char(c));
                    }
                    None => return Poll::Ready((self.consumed, None)),
                },
                State::TagOpen => match c {
                    Some('!') => todo!("markup decl state"),
                    Some('/') => {
                        self.consume(&mut chars);
                        self.state = State::EndTagOpen;
                    }
                    Some(c) if c.is_ascii_alphabetic() => {
                        self.str_buf.clear();
                        self.tok = Token::StartTag {
                            name: I::EMPTY_RANGE_INDEX,
                            attrs: I::EMPTY_RANGE_INDEX,
                            self_closing: false,
                        };
                        self.state = State::TagName;
                    }
                    None => return self.malformed_here("unexpected end of input"),
                    Some(c) => {
                        return self.malformed_here(format!("unexpected `{c}` instead of tag name"))
                    }
                },
                State::EndTagOpen => match c {
                    Some(c) if c.is_ascii_alphabetic() => {
                        self.str_buf.clear();
                        self.tok = Token::EndTag {
                            name: I::EMPTY_RANGE_INDEX,
                            attrs: I::EMPTY_RANGE_INDEX,
                        };
                        self.state = State::TagName;
                    }
                    Some('>') => return self.malformed_here("missing end tag name"),
                    None => return self.malformed_here("unexpected end of input"),
                    Some(c) => {
                        return self.malformed_here(format!("unexpected `{c}` instead of tag name"))
                    }
                },
                State::TagName => match c {
                    Some('\t' | '\n' | '\x0C' | ' ') => {
                        self.consume(&mut chars);
                        self.state = State::BeforeAttributeName;
                        self.set_tag_name_if_unset();
                    }
                    Some('/') => {
                        self.consume(&mut chars);
                        self.state = State::SelfClosingStartTag;
                    }
                    Some('>') => {
                        self.consume(&mut chars);
                        self.state = State::Data;
                        self.set_tag_name_if_unset();
                        self.set_tag_attrs_if_unset();
                        return self.token(self.start_loc, self.tok);
                    }
                    Some(c) if c.is_ascii_uppercase() => {
                        self.consume(&mut chars);
                        self.str_buf.push(c.to_ascii_lowercase());
                    }
                    Some('\x00') => return self.malformed_here("unexpected null byte"),
                    None => return self.malformed_here("unexpected end of input"),
                    Some(c) => {
                        self.consume(&mut chars);
                        self.str_buf.push(c);
                    }
                },
                State::BeforeAttributeName => match c {
                    Some('\t' | '\n' | '\x0C' | ' ') => {
                        self.consume(&mut chars);
                    }
                    None | Some('/' | '>') => self.state = State::AfterAttributeName,
                    Some('=') => {
                        return self.malformed_here("unexpected `=` before attribute name")
                    }
                    Some(_) => {
                        self.str_buf.clear();
                        self.attr_buf.clear();
                        self.attr_buf
                            .push([I::EMPTY_RANGE_INDEX, I::EMPTY_RANGE_INDEX]);
                        self.state = State::AttributeName;
                    }
                },
                State::AttributeName => match c {
                    None | Some('\t' | '\n' | '\x0C' | ' ' | '/' | '>') => {
                        let name = self.interner.intern_str(&self.str_buf);
                        self.attr_buf.last_mut().unwrap()[0] = name;
                        self.state = State::AfterAttributeName;
                    }
                    Some('=') => {
                        self.consume(&mut chars);
                        let name = self.interner.intern_str(&self.str_buf);
                        self.attr_buf.last_mut().unwrap()[0] = name;
                        self.state = State::BeforeAttributeValue;
                    }
                    Some(c) if c.is_ascii_uppercase() => {
                        self.consume(&mut chars);
                        self.str_buf.push(c.to_ascii_lowercase());
                    }
                    Some('\x00') => return self.malformed_here("unexpected null byte"),
                    Some(c @ '"' | c @ '\'' | c @ '<') => {
                        return self.malformed_here(format!("unexpected `{c}` in attribute name"))
                    }
                    Some(c) => {
                        self.consume(&mut chars);
                        self.str_buf.push(c);
                    }
                },
                State::AfterAttributeName => match c {
                    Some('\t' | '\n' | '\x0C' | ' ') => {
                        self.consume(&mut chars);
                    }
                    Some('/') => {
                        self.consume(&mut chars);
                        self.state = State::SelfClosingStartTag;
                    }
                    Some('=') => {
                        self.consume(&mut chars);
                        self.state = State::BeforeAttributeValue;
                    }
                    Some('>') => {
                        self.consume(&mut chars);
                        self.state = State::Data;
                    }
                    None => return self.malformed_here("unexpected end of input"),
                    Some(_) => {
                        self.str_buf.clear();
                        self.attr_buf
                            .push([I::EMPTY_RANGE_INDEX, I::EMPTY_RANGE_INDEX]);
                    }
                },
                State::BeforeAttributeValue => match c {
                    Some('\t' | '\n' | '\x0C' | ' ') => {
                        self.consume(&mut chars);
                    }
                    Some('"') => {
                        self.consume(&mut chars);
                        self.str_buf.clear();
                        self.state = State::AttributeValueDoubleQuote;
                    }
                    Some('\'') => {
                        self.consume(&mut chars);
                        self.str_buf.clear();
                        self.state = State::AttributeValueSingleQuote;
                    }
                    Some('>') => return self.malformed_here("missing attribute value"),
                    _ => {
                        self.state = State::AttributeValueNoQuote;
                        self.str_buf.clear();
                    }
                },
                State::AttributeValueDoubleQuote => match c {
                    Some('"') => {
                        self.consume(&mut chars);
                        let value = self.interner.intern_str(&self.str_buf);
                        self.attr_buf.last_mut().unwrap()[1] = value;
                        self.state = State::AfterAttributeValueQuoted;
                    }
                    Some('&') => todo!("char reference state"),
                    Some('\x00') => return self.malformed_here("unexpected null byte"),
                    None => return self.malformed_here("unexpected end of input"),
                    Some(c) => {
                        self.consume(&mut chars);
                        self.str_buf.push(c);
                    }
                },
                State::AttributeValueSingleQuote => match c {
                    Some('\'') => {
                        self.consume(&mut chars);
                        let value = self.interner.intern_str(&self.str_buf);
                        self.attr_buf.last_mut().unwrap()[1] = value;
                        self.state = State::AfterAttributeValueQuoted;
                    }
                    Some('&') => todo!("char reference state"),
                    Some('\x00') => return self.malformed_here("unexpected null byte"),
                    None => return self.malformed_here("unexpected end of input"),
                    Some(c) => {
                        self.consume(&mut chars);
                        self.str_buf.push(c);
                    }
                },
                State::AttributeValueNoQuote => match c {
                    Some('\t' | '\n' | '\x0C' | ' ') => {
                        self.consume(&mut chars);
                        let value = self.interner.intern_str(&self.str_buf);
                        self.attr_buf.last_mut().unwrap()[1] = value;
                        self.state = State::BeforeAttributeName;
                    }
                    Some('&') => todo!("char reference state"),
                    Some('>') => {
                        self.consume(&mut chars);
                        let value = self.interner.intern_str(&self.str_buf);
                        self.attr_buf.last_mut().unwrap()[1] = value;
                        self.set_tag_attrs_if_unset();
                        self.state = State::Data;
                        return self.token(self.start_loc, self.tok);
                    }
                    Some('\x00') => return self.malformed_here("unexpected null byte"),
                    Some(c @ '"' | c @ '\'' | c @ '<' | c @ '=' | c @ '`') => {
                        return self.malformed_here(format!("unexpected `{c}` in attribute value"));
                    }
                    None => return self.malformed_here("unexpected end of input"),
                    Some(c) => {
                        self.consume(&mut chars);
                        self.str_buf.push(c);
                    }
                },
                State::AfterAttributeValueQuoted => match c {
                    Some('\t' | '\n' | '\x0C' | ' ') => {
                        self.consume(&mut chars);
                        self.state = State::BeforeAttributeName;
                    }
                    Some('/') => {
                        self.consume(&mut chars);
                        self.state = State::SelfClosingStartTag;
                    }
                    Some('>') => {
                        self.consume(&mut chars);
                        self.state = State::Data;
                        self.set_tag_attrs_if_unset();
                        return self.token(self.start_loc, self.tok);
                    }
                    None => return self.malformed_here("unexpected end of input"),
                    Some(_) => return self.malformed_here("missing whitespace between attributes"),
                },
                State::SelfClosingStartTag => match c {
                    Some('>') => {
                        self.consume(&mut chars);
                        self.set_tag_name_if_unset();
                        self.set_tag_attrs_if_unset();
                        self.state = State::Data;
                        if let Token::StartTag { name, attrs, .. } = self.tok {
                            return self.token(
                                self.start_loc,
                                Token::StartTag {
                                    name,
                                    attrs,
                                    self_closing: true,
                                },
                            );
                        }
                        unreachable!()
                    }
                    None => return self.malformed_here("unexpected end of input"),
                    Some(_) => return self.malformed_here("unexpected `/` in tag"),
                },
            }
        }
    }
}

impl<R: AsyncRead + Unpin, I: Interner> Stream for Tokenizer<R, I> {
    type Item = TokenzizerItem;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();
        let input = {
            match this.reader.as_mut().poll_fill_buf(cx) {
                Poll::Ready(Ok(s)) => s,
                Poll::Ready(Err(err)) => {
                    return Poll::Ready(Some((this.inner.loc, Err(err.into()))));
                }
                Poll::Pending => {
                    cx.waker().wake_by_ref();
                    return Poll::Pending;
                }
            }
        };
        if let Poll::Ready((consumed, item)) = this.inner.next(input) {
            this.reader.consume(consumed);
            this.inner.consumed = 0;
            Poll::Ready(item)
        } else {
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

#[cfg(test)]
mod tests {
    use smol::io::Cursor;

    use super::*;
    use crate::io;

    fn cx<'a>() -> Context<'a> {
        Context::from_waker(io::noop_waker_ref())
    }

    struct MockInterner {
        strings: Vec<String>,
        attrs: Vec<Vec<[usize; 2]>>,
    }

    impl MockInterner {
        fn new() -> Self {
            Self {
                strings: vec!["".into()],
                attrs: vec![Vec::new()],
            }
        }
    }

    impl Interner for MockInterner {
        const EMPTY_RANGE_INDEX: usize = 0;
        fn intern_str(&mut self, s: &str) -> usize {
            if let Some(pos) = self.strings.iter().position(|string| s == string) {
                return pos;
            }
            let len = self.strings.len();
            self.strings.push(s.into());
            len
        }
        fn intern_attrs(&mut self, attrs: &[[usize; 2]]) -> usize {
            if let Some(pos) = self.attrs.iter().position(|attr| attr == attrs) {
                return pos;
            }
            let len = self.attrs.len();
            self.attrs.push(attrs.into());
            len
        }
    }

    fn assert_none<R: AsyncRead + Unpin, I: Interner>(
        cx: &mut Context<'_>,
        tokenizer: &mut Tokenizer<R, I>,
    ) {
        assert!(matches!(
            Pin::new(tokenizer).poll_next(cx),
            Poll::Ready(None)
        ));
    }

    fn assert_token<R: AsyncRead + Unpin, I: Interner, L: Into<Location>>(
        cx: &mut Context<'_>,
        tokenizer: &mut Tokenizer<R, I>,
        loc: L,
        tok: Token,
    ) {
        let result = Pin::new(tokenizer).poll_next(cx);
        assert!(matches!(result, Poll::Ready(Some((_, Ok(_))))));
        if let Poll::Ready(Some((location, Ok(token)))) = result {
            assert_eq!(loc.into(), location);
            assert_eq!(tok, token);
        }
    }

    #[test]
    fn empty() {
        let buf = AsyncStrReader::new(Cursor::new(""));
        let int = MockInterner::new();
        let mut cx = cx();
        let mut tok = Tokenizer::new(buf, int);
        assert_none(&mut cx, &mut tok);
    }

    #[test]
    fn char() {
        let buf = AsyncStrReader::new(Cursor::new("abc"));
        let int = MockInterner::new();
        let mut cx = cx();
        let mut tok = Tokenizer::new(buf, int);
        assert_token(&mut cx, &mut tok, [1, 1], Token::Char('a'));
        assert_token(&mut cx, &mut tok, [1, 2], Token::Char('b'));
        assert_token(&mut cx, &mut tok, [1, 3], Token::Char('c'));
        assert_none(&mut cx, &mut tok);
    }

    #[test]
    fn start_tag() {
        let buf = AsyncStrReader::new(Cursor::new("<hello>"));
        let int = MockInterner::new();
        let mut cx = cx();
        let mut tok = Tokenizer::new(buf, int);
        assert_token(
            &mut cx,
            &mut tok,
            [1, 1],
            Token::StartTag {
                name: 1,
                attrs: MockInterner::EMPTY_RANGE_INDEX,
                self_closing: false,
            },
        );
        assert_none(&mut cx, &mut tok);
    }

    #[test]
    fn end_tag() {
        let buf = AsyncStrReader::new(Cursor::new("</hello>"));
        let int = MockInterner::new();
        let mut cx = cx();
        let mut tok = Tokenizer::new(buf, int);
        assert_token(
            &mut cx,
            &mut tok,
            [1, 1],
            Token::EndTag {
                name: 1,
                attrs: MockInterner::EMPTY_RANGE_INDEX,
            },
        );
        assert_none(&mut cx, &mut tok);
    }

    #[test]
    fn self_closing_tag() {
        let buf = AsyncStrReader::new(Cursor::new("<hello/>"));
        let int = MockInterner::new();
        let mut cx = cx();
        let mut tok = Tokenizer::new(buf, int);
        assert_token(
            &mut cx,
            &mut tok,
            [1, 1],
            Token::StartTag {
                name: 1,
                attrs: MockInterner::EMPTY_RANGE_INDEX,
                self_closing: true,
            },
        );
        assert_none(&mut cx, &mut tok);
    }

    #[test]
    fn start_tag_attrs() {
        let buf = AsyncStrReader::new(Cursor::new(
            "<hello key='test'><hello key=\"test\"><hello key=test>",
        ));
        let int = MockInterner::new();
        let mut cx = cx();
        let mut tok = Tokenizer::new(buf, int);
        assert_token(
            &mut cx,
            &mut tok,
            [1, 1],
            Token::StartTag {
                name: 1,
                attrs: 1,
                self_closing: false,
            },
        );
        assert_token(
            &mut cx,
            &mut tok,
            [1, 19],
            Token::StartTag {
                name: 1,
                attrs: 1,
                self_closing: false,
            },
        );
        assert_token(
            &mut cx,
            &mut tok,
            [1, 37],
            Token::StartTag {
                name: 1,
                attrs: 1,
                self_closing: false,
            },
        );
        assert_none(&mut cx, &mut tok);
    }
}
