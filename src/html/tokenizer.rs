use std::{
    io,
    pin::Pin,
    str,
    task::{Context, Poll},
};

use smol::io::AsyncRead;

use crate::io::{AsyncStrError, AsyncStrReader, Location, NewlineNormalizable};

#[derive(thiserror::Error, Debug)]
pub enum TokenizerError {
    #[error(transparent)]
    IoError(#[from] io::Error),

    #[error(transparent)]
    Utf8Error(#[from] str::Utf8Error),
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
    },
    DocType,
    Comment,
}

pub enum State {
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

    BogusComment,
    RcData,
    RcDataLessThan,
    RcDataEndTagOpen,
    RcDataEndTagName,
}

pub trait Interner {
    const EMPTY_RANGE_INDEX: usize;
    fn intern_str(&mut self, s: &str) -> usize;
    fn intern_attrs(&mut self, attrs: &[[usize; 2]]) -> usize;
}

struct TokenizerInner {
    consumed: usize,
    state: State,
    str_buf: String,
    attr_buf: Vec<[usize; 2]>,
    temp_buffer: Vec<(Location, char)>,
    last_start_tag_emitted_name: Option<usize>,
    synthetic_toks: Vec<(Location, Token)>,
    force_eof: bool,
    tok: Token,
    start_loc: Location,
    loc: Location,
}

#[must_use]
#[pin_project::pin_project]
pub struct Tokenizer<R> {
    #[pin]
    reader: AsyncStrReader<R>,
    inner: TokenizerInner,
}

impl<R> Tokenizer<R> {
    pub fn new(reader: AsyncStrReader<R>) -> Self {
        let inner = TokenizerInner {
            consumed: 0,
            state: State::Data,
            str_buf: String::new(),
            attr_buf: Vec::new(),
            temp_buffer: Vec::new(),
            last_start_tag_emitted_name: None,
            synthetic_toks: Vec::new(),
            force_eof: false,
            tok: Token::Comment,
            start_loc: Location { line: 1, column: 1 },
            loc: Location { line: 1, column: 0 },
        };
        Self { reader, inner }
    }

    pub fn set_state(&mut self, state: State) {
        self.inner.state = state;
    }
}

type TokenzizerItem = (Location, Result<Token, TokenizerError>);

impl TokenizerInner {
    fn token(&mut self, loc: Location, tok: Token) -> Poll<Option<TokenzizerItem>> {
        Poll::Ready(Some((loc, Ok(tok))))
    }

    fn token_here(&mut self, tok: Token) -> Poll<Option<TokenzizerItem>> {
        self.token(self.loc, tok)
    }

    fn set_tag_name_if_unset<I: Interner>(&mut self, int: &mut I) {
        self.tok = match self.tok {
            Token::StartTag {
                name,
                attrs,
                self_closing,
            } if name == I::EMPTY_RANGE_INDEX => {
                let name = int.intern_str(&self.str_buf);
                Token::StartTag {
                    name,
                    attrs,
                    self_closing,
                }
            }
            Token::EndTag { name } if name == I::EMPTY_RANGE_INDEX => {
                let name = int.intern_str(&self.str_buf);
                Token::EndTag { name }
            }
            tok => tok,
        }
    }

    fn set_tag_attrs_if_unset<I: Interner>(&mut self, int: &mut I) {
        self.tok = match self.tok {
            Token::StartTag {
                name,
                attrs,
                self_closing,
            } if attrs == I::EMPTY_RANGE_INDEX => {
                let attrs = int.intern_attrs(&self.attr_buf);
                Token::StartTag {
                    name,
                    attrs,
                    self_closing,
                }
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

    fn next<I: Interner>(&mut self, input: &str, int: &mut I) -> Poll<Option<TokenzizerItem>> {
        // TODO: try changing the code to match the parser. we dont need to project and
        //   have an inner field that we split out.
        let mut chars = input.chars().newline_normalized().peekable();
        loop {
            let c = chars.peek().map(|(_, c)| *c);
            if c.is_none() && !input.is_empty() {
                return Poll::Pending;
            }
            match self.state {
                State::Data => match c {
                    Some('&') => todo!("char reference state"),
                    Some('<') => {
                        self.consume(&mut chars);
                        self.state = State::TagOpen;
                        self.start_loc = self.loc;
                    }
                    Some('\x00') => {
                        // error: unexpected-null-character
                        self.consume(&mut chars);
                        return self.token_here(Token::Char('\x00'));
                    }
                    Some(c) => {
                        self.consume(&mut chars);
                        return self.token_here(Token::Char(c));
                    }
                    None => return Poll::Ready(None),
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
                    Some('?') => {
                        // error: unexpected-question-mark-instead-of-tag-name
                        self.state = State::BogusComment;
                    }
                    None => {
                        // error: eof-before-tag-name
                        self.force_eof = true;
                        return self.token_here(Token::Char('<'));
                    }
                    Some(_) => {
                        // error: invalid-first-character-of-tag-name
                        self.state = State::Data;
                        return self.token_here(Token::Char('<'));
                    }
                },
                State::EndTagOpen => match c {
                    Some(c) if c.is_ascii_alphabetic() => {
                        self.str_buf.clear();
                        self.tok = Token::EndTag {
                            name: I::EMPTY_RANGE_INDEX,
                        };
                        self.state = State::TagName;
                    }
                    Some('>') => {
                        // error: missing-end-tag-name
                        self.consume(&mut chars);
                        self.state = State::Data;
                    }
                    None => {
                        // error: eof-before-tag-name
                        self.force_eof = true;
                        self.synthetic_toks.push((self.loc, Token::Char('/')));
                        return self.token(self.start_loc, Token::Char('<'));
                    }
                    Some(_) => {
                        // error: invalid-first-character-of-tag-name
                        self.state = State::BogusComment;
                    }
                },
                State::TagName => match c {
                    Some('\t' | '\n' | '\x0C' | ' ') => {
                        self.consume(&mut chars);
                        self.state = State::BeforeAttributeName;
                        self.set_tag_name_if_unset(int);
                    }
                    Some('/') => {
                        self.consume(&mut chars);
                        self.state = State::SelfClosingStartTag;
                    }
                    Some('>') => {
                        self.consume(&mut chars);
                        self.state = State::Data;
                        self.set_tag_name_if_unset(int);
                        self.set_tag_attrs_if_unset(int);
                        self.attr_buf.clear();
                        return self.token(self.start_loc, self.tok);
                    }
                    Some(c) if c.is_ascii_uppercase() => {
                        self.consume(&mut chars);
                        self.str_buf.push(c.to_ascii_lowercase());
                    }
                    Some('\x00') => {
                        // error: unexpected-null-character
                        self.consume(&mut chars);
                        self.str_buf.push(char::REPLACEMENT_CHARACTER);
                    }
                    None => {
                        // error: eof-in-tag
                        return Poll::Ready(None);
                    }
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
                        // error: unexpected-equals-sign-before-attribute-name
                        self.consume(&mut chars);
                        self.str_buf.clear();
                        self.str_buf.push('=');
                        self.attr_buf
                            .push([I::EMPTY_RANGE_INDEX, I::EMPTY_RANGE_INDEX]);
                        self.state = State::AttributeName;
                    }
                    Some(_) => {
                        self.str_buf.clear();
                        self.attr_buf
                            .push([I::EMPTY_RANGE_INDEX, I::EMPTY_RANGE_INDEX]);
                        self.state = State::AttributeName;
                    }
                },
                State::AttributeName => match c {
                    None | Some('\t' | '\n' | '\x0C' | ' ' | '/' | '>') => {
                        let name = int.intern_str(&self.str_buf);
                        self.attr_buf.last_mut().unwrap()[0] = name;
                        self.state = State::AfterAttributeName;
                    }
                    Some('=') => {
                        self.consume(&mut chars);
                        let name = int.intern_str(&self.str_buf);
                        self.attr_buf.last_mut().unwrap()[0] = name;
                        self.state = State::BeforeAttributeValue;
                    }
                    Some(c) if c.is_ascii_uppercase() => {
                        self.consume(&mut chars);
                        self.str_buf.push(c.to_ascii_lowercase());
                    }
                    Some('\x00') => {
                        self.state = State::BeforeAttributeValue;
                    }
                    Some(c @ '"' | c @ '\'' | c @ '<') => {
                        // error: unexpected-character-in-attribute-name
                        self.consume(&mut chars);
                        self.str_buf.push(c);
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
                    None => {
                        // error: eof-in-tag
                        return Poll::Ready(None);
                    }
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
                    Some('>') => {
                        // error: missing-attribute-value
                        self.consume(&mut chars);
                        self.set_tag_attrs_if_unset(int);
                        self.attr_buf.clear();
                        self.state = State::Data;
                        return self.token(self.start_loc, self.tok);
                    }
                    _ => {
                        self.state = State::AttributeValueNoQuote;
                        self.str_buf.clear();
                    }
                },
                State::AttributeValueDoubleQuote => match c {
                    Some('"') => {
                        self.consume(&mut chars);
                        let value = int.intern_str(&self.str_buf);
                        self.attr_buf.last_mut().unwrap()[1] = value;
                        self.state = State::AfterAttributeValueQuoted;
                    }
                    Some('&') => todo!("char reference state"),
                    Some('\x00') => {
                        // error: unexpected-null-character
                        self.consume(&mut chars);
                        self.str_buf.push(char::REPLACEMENT_CHARACTER);
                    }
                    None => {
                        // error: eof-in-tag
                        return Poll::Ready(None);
                    }
                    Some(c) => {
                        self.consume(&mut chars);
                        self.str_buf.push(c);
                    }
                },
                State::AttributeValueSingleQuote => match c {
                    Some('\'') => {
                        self.consume(&mut chars);
                        let value = int.intern_str(&self.str_buf);
                        self.attr_buf.last_mut().unwrap()[1] = value;
                        self.state = State::AfterAttributeValueQuoted;
                    }
                    Some('&') => todo!("char reference state"),
                    Some('\x00') => {
                        // error: unexpected-null-character
                        self.consume(&mut chars);
                        self.str_buf.push(char::REPLACEMENT_CHARACTER);
                    }
                    None => {
                        // error: eof-in-tag
                        return Poll::Ready(None);
                    }
                    Some(c) => {
                        self.consume(&mut chars);
                        self.str_buf.push(c);
                    }
                },
                State::AttributeValueNoQuote => match c {
                    Some('\t' | '\n' | '\x0C' | ' ') => {
                        self.consume(&mut chars);
                        let value = int.intern_str(&self.str_buf);
                        self.attr_buf.last_mut().unwrap()[1] = value;
                        self.state = State::BeforeAttributeName;
                    }
                    Some('&') => todo!("char reference state"),
                    Some('>') => {
                        self.consume(&mut chars);
                        let value = int.intern_str(&self.str_buf);
                        self.attr_buf.last_mut().unwrap()[1] = value;
                        self.set_tag_attrs_if_unset(int);
                        self.attr_buf.clear();
                        self.state = State::Data;
                        return self.token(self.start_loc, self.tok);
                    }
                    Some('\x00') => {
                        // error: unexpected-null-character
                        self.consume(&mut chars);
                        self.str_buf.push(char::REPLACEMENT_CHARACTER);
                    }
                    Some(c @ '"' | c @ '\'' | c @ '<' | c @ '=' | c @ '`') => {
                        // error: unexpected-character-in-unquoted-attribute-value
                        self.consume(&mut chars);
                        self.str_buf.push(c);
                    }
                    None => {
                        // error: eof-in-tag
                        return Poll::Ready(None);
                    }
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
                        self.set_tag_attrs_if_unset(int);
                        self.attr_buf.clear();
                        return self.token(self.start_loc, self.tok);
                    }
                    None => {
                        // error: eof-in-tag
                        return Poll::Ready(None);
                    }
                    Some(_) => {
                        // error: missing-whitespace-between-attributes
                        self.state = State::BeforeAttributeName;
                    }
                },
                State::SelfClosingStartTag => match c {
                    Some('>') => {
                        self.consume(&mut chars);
                        self.set_tag_name_if_unset(int);
                        self.set_tag_attrs_if_unset(int);
                        self.attr_buf.clear();
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
                    None => {
                        // error: eof-in-tag
                        return Poll::Ready(None);
                    }
                    Some(_) => {
                        // error: unexpected-solidus-in-tag
                        self.state = State::BeforeAttributeName;
                    }
                },
                State::BogusComment => todo!("bogus comment"),
                State::RcData => match c {
                    Some('&') => todo!("char reference state"),
                    Some('<') => {
                        self.consume(&mut chars);
                        self.start_loc = self.loc;
                        self.state = State::RcDataLessThan;
                    }
                    Some('\x00') => {
                        // error: unexpected-null-character
                        self.consume(&mut chars);
                        return self.token_here(Token::Char(char::REPLACEMENT_CHARACTER));
                    }
                    None => return Poll::Ready(None),
                    Some(c) => {
                        self.consume(&mut chars);
                        return self.token_here(Token::Char(c));
                    }
                },
                State::RcDataLessThan => match c {
                    Some('/') => {
                        self.consume(&mut chars);
                        self.temp_buffer.clear();
                        self.state = State::RcDataEndTagOpen;
                    }
                    _ => {
                        self.state = State::RcData;
                        return self.token(self.start_loc, Token::Char('<'));
                    }
                },
                State::RcDataEndTagOpen => match c {
                    Some(c) if c.is_alphabetic() => {
                        self.str_buf.clear();
                        self.tok = Token::EndTag {
                            name: I::EMPTY_RANGE_INDEX,
                        };
                        self.state = State::RcDataEndTagName;
                    }
                    _ => {
                        self.synthetic_toks.push((self.loc, Token::Char('/')));
                        self.state = State::RcData;
                        return self.token(self.start_loc, Token::Char('<'));
                    }
                },
                State::RcDataEndTagName => match c {
                    Some('\t' | '\n' | '\x0C' | ' ') => {
                        self.set_tag_name_if_unset(int);
                        if let Token::EndTag { name } = self.tok {
                            if matches!(self.last_start_tag_emitted_name, Some(n) if n == name) {
                                self.consume(&mut chars);
                                self.state = State::BeforeAttributeName;
                                return Poll::Pending;
                            }
                        }
                        for (loc, c) in self.temp_buffer.drain(..).rev() {
                            self.synthetic_toks.push((loc, Token::Char(c)));
                        }
                        self.synthetic_toks.push((self.loc, Token::Char('/')));
                        self.state = State::RcData;
                        return self.token(self.start_loc, Token::Char('<'));
                    }
                    Some('/') => {
                        self.set_tag_name_if_unset(int);
                        if let Token::EndTag { name } = self.tok {
                            if matches!(self.last_start_tag_emitted_name, Some(n) if n == name) {
                                self.consume(&mut chars);
                                self.state = State::SelfClosingStartTag;
                                return Poll::Pending;
                            }
                        }
                        for (loc, c) in self.temp_buffer.drain(..).rev() {
                            self.synthetic_toks.push((loc, Token::Char(c)));
                        }
                        self.synthetic_toks.push((self.loc, Token::Char('/')));
                        self.state = State::RcData;
                        return self.token(self.start_loc, Token::Char('<'));
                    }
                    Some('>') => {
                        self.set_tag_name_if_unset(int);
                        if let Token::EndTag { name } = self.tok {
                            if matches!(self.last_start_tag_emitted_name, Some(n) if n == name) {
                                self.consume(&mut chars);
                                self.state = State::Data;
                                return self.token(self.start_loc, self.tok);
                            }
                        }
                        for (loc, c) in self.temp_buffer.drain(..).rev() {
                            self.synthetic_toks.push((loc, Token::Char(c)));
                        }
                        self.synthetic_toks.push((self.loc, Token::Char('/')));
                        self.state = State::RcData;
                        return self.token(self.start_loc, Token::Char('<'));
                    }
                    Some(c) if c.is_ascii_uppercase() => {
                        self.consume(&mut chars);
                        self.str_buf.push(c.to_ascii_lowercase());
                        self.temp_buffer.push((self.loc, c));
                    }
                    Some(c) if c.is_ascii_lowercase() => {
                        self.consume(&mut chars);
                        self.str_buf.push(c);
                        self.temp_buffer.push((self.loc, c));
                    }
                    _ => {
                        for (loc, c) in self.temp_buffer.drain(..).rev() {
                            self.synthetic_toks.push((loc, Token::Char(c)));
                        }
                        self.synthetic_toks.push((self.loc, Token::Char('/')));
                        self.state = State::RcData;
                        return self.token(self.start_loc, Token::Char('<'));
                    }
                },
            }
        }
    }
}

impl<R: AsyncRead + Unpin> Tokenizer<R> {
    pub fn poll_next<I: Interner>(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        int: &mut I,
    ) -> Poll<Option<TokenzizerItem>> {
        let mut this = self.project();
        if let Some((loc, tok)) = this.inner.synthetic_toks.pop() {
            if let Token::StartTag { name, .. } = tok {
                this.inner.last_start_tag_emitted_name = Some(name);
            }
            return Poll::Ready(Some((loc, Ok(tok))));
        }
        if this.inner.force_eof {
            return Poll::Ready(None);
        }
        let input = {
            match this.reader.as_mut().poll_fill_buf(cx) {
                Poll::Ready(Ok(s)) => s,
                Poll::Ready(Err(err)) => {
                    // TODO: could handle some io errors as <EOF>
                    return Poll::Ready(Some((this.inner.loc, Err(err.into()))));
                }
                Poll::Pending => {
                    cx.waker().wake_by_ref();
                    return Poll::Pending;
                }
            }
        };
        match this.inner.next(input, int) {
            Poll::Ready(item) => {
                if let Some((_, Ok(Token::StartTag { name, .. }))) = item {
                    this.inner.last_start_tag_emitted_name = Some(name);
                }
                this.reader.consume(this.inner.consumed);
                this.inner.consumed = 0;
                Poll::Ready(item)
            }
            Poll::Pending => {
                this.reader.consume(this.inner.consumed);
                this.inner.consumed = 0;
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use smol::io::Cursor;

    use super::*;
    use crate::asyncro;

    fn cx<'a>() -> Context<'a> {
        Context::from_waker(asyncro::noop_waker_ref())
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
        tokenizer: &mut Tokenizer<R>,
        int: &mut I,
    ) {
        assert!(matches!(
            Pin::new(tokenizer).poll_next(cx, int),
            Poll::Ready(None)
        ));
    }

    fn assert_pending<R: AsyncRead + Unpin, I: Interner>(
        cx: &mut Context<'_>,
        tokenizer: &mut Tokenizer<R>,
        int: &mut I,
    ) {
        assert!(matches!(
            Pin::new(tokenizer).poll_next(cx, int),
            Poll::Pending
        ));
    }

    fn assert_token<R: AsyncRead + Unpin, I: Interner, L: Into<Location>>(
        cx: &mut Context<'_>,
        tokenizer: &mut Tokenizer<R>,
        int: &mut I,
        loc: L,
        tok: Token,
    ) {
        let result = Pin::new(tokenizer).poll_next(cx, int);
        assert!(matches!(result, Poll::Ready(Some((_, Ok(_))))));
        if let Poll::Ready(Some((location, Ok(token)))) = result {
            assert_eq!(loc.into(), location);
            assert_eq!(tok, token);
        }
    }

    fn assert_str(int: &MockInterner, s: &str, index: usize) {
        assert_eq!(&int.strings[index], s);
    }

    fn assert_attrs(int: &MockInterner, attrs: &[[&str; 2]], index: usize) {
        for (lhs, rhs) in int.attrs[index].iter().zip(attrs) {
            assert_eq!(&int.strings[lhs[0]], rhs[0]);
            assert_eq!(&int.strings[lhs[1]], rhs[1]);
        }
    }

    #[test]
    fn empty() {
        let buf = AsyncStrReader::new(Cursor::new(""));
        let mut int = MockInterner::new();
        let mut cx = cx();
        let mut tok = Tokenizer::new(buf);
        assert_none(&mut cx, &mut tok, &mut int);
    }

    #[test]
    fn char() {
        let buf = AsyncStrReader::new(Cursor::new("abc"));
        let mut int = MockInterner::new();
        let mut cx = cx();
        let mut tok = Tokenizer::new(buf);
        assert_token(&mut cx, &mut tok, &mut int, [1, 1], Token::Char('a'));
        assert_token(&mut cx, &mut tok, &mut int, [1, 2], Token::Char('b'));
        assert_token(&mut cx, &mut tok, &mut int, [1, 3], Token::Char('c'));
        assert_none(&mut cx, &mut tok, &mut int);
    }

    #[test]
    fn start_tag() {
        let buf = AsyncStrReader::new(Cursor::new("<hello>"));
        let mut int = MockInterner::new();
        let mut cx = cx();
        let mut tok = Tokenizer::new(buf);
        assert_token(
            &mut cx,
            &mut tok,
            &mut int,
            [1, 1],
            Token::StartTag {
                name: 1,
                attrs: MockInterner::EMPTY_RANGE_INDEX,
                self_closing: false,
            },
        );
        assert_none(&mut cx, &mut tok, &mut int);
        assert_str(&mut int, "hello", 1);
    }

    #[test]
    fn end_tag() {
        let buf = AsyncStrReader::new(Cursor::new("</hello>"));
        let mut int = MockInterner::new();
        let mut cx = cx();
        let mut tok = Tokenizer::new(buf);
        assert_token(
            &mut cx,
            &mut tok,
            &mut int,
            [1, 1],
            Token::EndTag { name: 1 },
        );
        assert_none(&mut cx, &mut tok, &mut int);
        assert_str(&mut int, "hello", 1);
    }

    #[test]
    fn self_closing_tag() {
        let buf = AsyncStrReader::new(Cursor::new("<hello/>"));
        let mut int = MockInterner::new();
        let mut cx = cx();
        let mut tok = Tokenizer::new(buf);
        assert_token(
            &mut cx,
            &mut tok,
            &mut int,
            [1, 1],
            Token::StartTag {
                name: 1,
                attrs: MockInterner::EMPTY_RANGE_INDEX,
                self_closing: true,
            },
        );
        assert_none(&mut cx, &mut tok, &mut int);
        assert_str(&mut int, "hello", 1);
    }

    #[test]
    fn start_tag_attrs() {
        let buf = AsyncStrReader::new(Cursor::new(
            "<hello key='test'><hello key=\"test\"><hello key=test>",
        ));
        let mut int = MockInterner::new();
        let mut cx = cx();
        let mut tok = Tokenizer::new(buf);
        assert_token(
            &mut cx,
            &mut tok,
            &mut int,
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
            &mut int,
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
            &mut int,
            [1, 37],
            Token::StartTag {
                name: 1,
                attrs: 1,
                self_closing: false,
            },
        );
        assert_none(&mut cx, &mut tok, &mut int);
        assert_str(&mut int, "hello", 1);
        assert_attrs(&mut int, &[["key", "test"]], 1);
    }

    #[test]
    fn error_unexpected_null() {
        let buf = AsyncStrReader::new(Cursor::new("\x00"));
        let mut int = MockInterner::new();
        let mut cx = cx();
        let mut tok = Tokenizer::new(buf);
        assert_token(&mut cx, &mut tok, &mut int, [1, 1], Token::Char('\x00'));
        assert_none(&mut cx, &mut tok, &mut int);
    }

    #[test]
    fn error_eof_before_tag_name() {
        let buf = AsyncStrReader::new(Cursor::new("<"));
        let mut int = MockInterner::new();
        let mut cx = cx();
        let mut tok = Tokenizer::new(buf);
        assert_pending(&mut cx, &mut tok, &mut int);
        assert_token(&mut cx, &mut tok, &mut int, [1, 1], Token::Char('<'));
        assert_none(&mut cx, &mut tok, &mut int);
    }

    #[test]
    fn error_invalid_first_character_of_tag_name() {
        let buf = AsyncStrReader::new(Cursor::new("<3>"));
        let mut int = MockInterner::new();
        let mut cx = cx();
        let mut tok = Tokenizer::new(buf);
        assert_token(&mut cx, &mut tok, &mut int, [1, 1], Token::Char('<'));
        assert_token(&mut cx, &mut tok, &mut int, [1, 2], Token::Char('3'));
        assert_token(&mut cx, &mut tok, &mut int, [1, 3], Token::Char('>'));
        assert_none(&mut cx, &mut tok, &mut int);
    }

    #[test]
    fn error_missing_end_tag_name() {
        let buf = AsyncStrReader::new(Cursor::new("</>"));
        let mut int = MockInterner::new();
        let mut cx = cx();
        let mut tok = Tokenizer::new(buf);
        assert_pending(&mut cx, &mut tok, &mut int);
        assert_none(&mut cx, &mut tok, &mut int);
    }

    #[test]
    fn error_eof_before_tag_name_2() {
        let buf = AsyncStrReader::new(Cursor::new("</"));
        let mut int = MockInterner::new();
        let mut cx = cx();
        let mut tok = Tokenizer::new(buf);
        assert_pending(&mut cx, &mut tok, &mut int);
        assert_token(&mut cx, &mut tok, &mut int, [1, 1], Token::Char('<'));
        assert_token(&mut cx, &mut tok, &mut int, [1, 2], Token::Char('/'));
        assert_none(&mut cx, &mut tok, &mut int);
    }

    #[test]
    fn error_unexpected_null_2() {
        let buf = AsyncStrReader::new(Cursor::new("<test\x00>"));
        let mut int = MockInterner::new();
        let mut cx = cx();
        let mut tok = Tokenizer::new(buf);
        assert_token(
            &mut cx,
            &mut tok,
            &mut int,
            [1, 1],
            Token::StartTag {
                name: 1,
                attrs: MockInterner::EMPTY_RANGE_INDEX,
                self_closing: false,
            },
        );
        assert_none(&mut cx, &mut tok, &mut int);
        assert_str(&mut int, "test�", 1);
    }

    #[test]
    fn error_eof_in_tag() {
        let buf = AsyncStrReader::new(Cursor::new("<t"));
        let mut int = MockInterner::new();
        let mut cx = cx();
        let mut tok = Tokenizer::new(buf);
        assert_pending(&mut cx, &mut tok, &mut int);
        assert_none(&mut cx, &mut tok, &mut int);
    }

    #[test]
    fn error_unexpected_equals_sign_before_attribute_name() {
        let buf = AsyncStrReader::new(Cursor::new("<test ==foo>"));
        let mut int = MockInterner::new();
        let mut cx = cx();
        let mut tok = Tokenizer::new(buf);
        assert_token(
            &mut cx,
            &mut tok,
            &mut int,
            [1, 1],
            Token::StartTag {
                name: 1,
                attrs: 1,
                self_closing: false,
            },
        );
        assert_none(&mut cx, &mut tok, &mut int);
        assert_str(&mut int, "test", 1);
        assert_attrs(&mut int, &[["=", "foo"]], 1);
    }

    #[test]
    fn error_unexpected_character_in_attribute_name() {
        let buf = AsyncStrReader::new(Cursor::new("<test \"'<=foo>"));
        let mut int = MockInterner::new();
        let mut cx = cx();
        let mut tok = Tokenizer::new(buf);
        assert_token(
            &mut cx,
            &mut tok,
            &mut int,
            [1, 1],
            Token::StartTag {
                name: 1,
                attrs: 1,
                self_closing: false,
            },
        );
        assert_none(&mut cx, &mut tok, &mut int);
        assert_str(&mut int, "test", 1);
        assert_attrs(&mut int, &[["\"'<", "foo"]], 1);
    }

    #[test]
    fn error_eof_in_tag_2() {
        let buf = AsyncStrReader::new(Cursor::new("<test foo="));
        let mut int = MockInterner::new();
        let mut cx = cx();
        let mut tok = Tokenizer::new(buf);
        assert_pending(&mut cx, &mut tok, &mut int);
        assert_none(&mut cx, &mut tok, &mut int);
    }

    #[test]
    fn error_missing_attribute_value() {
        let buf = AsyncStrReader::new(Cursor::new("<test foo=>"));
        let mut int = MockInterner::new();
        let mut cx = cx();
        let mut tok = Tokenizer::new(buf);
        assert_token(
            &mut cx,
            &mut tok,
            &mut int,
            [1, 1],
            Token::StartTag {
                name: 1,
                attrs: 1,
                self_closing: false,
            },
        );
        assert_none(&mut cx, &mut tok, &mut int);
        assert_str(&mut int, "test", 1);
        assert_attrs(&mut int, &[["foo", ""]], 1);
    }

    #[test]
    fn error_unexpected_null_3() {
        let buf = AsyncStrReader::new(Cursor::new("<test foo=\"\x00\">"));
        let mut int = MockInterner::new();
        let mut cx = cx();
        let mut tok = Tokenizer::new(buf);
        assert_token(
            &mut cx,
            &mut tok,
            &mut int,
            [1, 1],
            Token::StartTag {
                name: 1,
                attrs: 1,
                self_closing: false,
            },
        );
        assert_none(&mut cx, &mut tok, &mut int);
        assert_str(&mut int, "test", 1);
        assert_attrs(&mut int, &[["foo", "�"]], 1);
    }

    #[test]
    fn error_eof_in_tag_3() {
        let buf = AsyncStrReader::new(Cursor::new("<test foo=\""));
        let mut int = MockInterner::new();
        let mut cx = cx();
        let mut tok = Tokenizer::new(buf);
        assert_pending(&mut cx, &mut tok, &mut int);
        assert_none(&mut cx, &mut tok, &mut int);
    }

    #[test]
    fn error_missing_whitespace_between_attributes() {
        let buf = AsyncStrReader::new(Cursor::new("<test foo=\"bar\"bar=\"baz\">"));
        let mut int = MockInterner::new();
        let mut cx = cx();
        let mut tok = Tokenizer::new(buf);
        assert_token(
            &mut cx,
            &mut tok,
            &mut int,
            [1, 1],
            Token::StartTag {
                name: 1,
                attrs: 1,
                self_closing: false,
            },
        );
        assert_none(&mut cx, &mut tok, &mut int);
        assert_str(&mut int, "test", 1);
        assert_attrs(&mut int, &[["foo", "bar"], ["bar", "baz"]], 1);
    }

    #[test]
    fn error_eof_in_tag_4() {
        let buf = AsyncStrReader::new(Cursor::new("</test"));
        let mut int = MockInterner::new();
        let mut cx = cx();
        let mut tok = Tokenizer::new(buf);
        assert_pending(&mut cx, &mut tok, &mut int);
        assert_none(&mut cx, &mut tok, &mut int);
    }

    #[test]
    fn error_unexpected_solidus_in_tag() {
        let buf = AsyncStrReader::new(Cursor::new("<test//>"));
        let mut int = MockInterner::new();
        let mut cx = cx();
        let mut tok = Tokenizer::new(buf);
        assert_token(
            &mut cx,
            &mut tok,
            &mut int,
            [1, 1],
            Token::StartTag {
                name: 1,
                attrs: MockInterner::EMPTY_RANGE_INDEX,
                self_closing: true,
            },
        );
        assert_none(&mut cx, &mut tok, &mut int);
        assert_str(&mut int, "test", 1);
    }
}
