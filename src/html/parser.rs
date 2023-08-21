use std::{
    pin::Pin,
    task::{Context, Poll},
};

use smol::{lock::BarrierWaitResult, prelude::AsyncRead};

use super::{
    tokenizer::{Interner, Token, Tokenizer, TokenizerError},
    State,
};
use crate::{
    dom::{Dom, EMPTY_RANGE_INDEX, ROOT_NODE_ID},
    io::{AsyncStrReader, Location},
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
    Text,
}

#[must_use]
#[pin_project::pin_project]
pub struct Parser<R> {
    #[pin]
    tokenizer: Tokenizer<R>,
    insertion_mode: InsertionMode,
    original_insertion_mode: InsertionMode,
    template_insertion_modes: Vec<InsertionMode>,
    stack: Vec<usize>,
    head: Option<usize>,
    text_buf: String,
    tok_buf: Vec<(Location, Token)>,
    frameset_ok: bool,
    skip_next_linefeed: bool,
}

impl Interner for Dom {
    const EMPTY_RANGE_INDEX: usize = EMPTY_RANGE_INDEX;
    fn intern_str(&mut self, s: &str) -> usize {
        self.insert_str(s)
    }
    fn intern_attrs(&mut self, attrs: &[[usize; 2]]) -> usize {
        self.insert_attrs(attrs)
    }
}

pub enum ParseEvent {
    Done,
    Fatal(Location, TokenizerError),
    Title(usize),
    Link, // TODO: need to fire off when a link tag is ready to fetch
    Style(usize),
    IFrame, // TODO
}

impl<R> Parser<R> {
    pub fn new(reader: AsyncStrReader<R>) -> Self {
        Self {
            tokenizer: Tokenizer::new(reader),
            insertion_mode: InsertionMode::Initial,
            original_insertion_mode: InsertionMode::Initial,
            template_insertion_modes: Vec::new(),
            stack: Vec::new(),
            head: None,
            text_buf: String::new(),
            tok_buf: Vec::new(),
            frameset_ok: true,
            skip_next_linefeed: false,
        }
    }

    fn is_str_in(&self, dom: &Dom, index: usize, strings: &[&str]) -> bool {
        for &string in strings {
            let string = dom.find_str(string);
            if matches!(string, Some(i) if i == index) {
                return true;
            }
        }
        false
    }

    fn append_text(&mut self, dom: &mut Dom, c: char) {
        dom.append_char(c);
        let top = *self.stack.last().unwrap();
        let element = dom.get_element_node(top).unwrap();
        // if the last child is a text node, then update its text
        if let Some(child) = element.child_indices().last() {
            if let Some(child) = dom.get_node_id_by_index(child) {
                if let Some(mut text) = dom.get_text_node_mut(child) {
                    self.text_buf.push(c);
                    text.set_text(&self.text_buf);
                    return;
                }
            }
        }
        // otherwise create a new child and set the text
        self.text_buf.clear();
        self.text_buf.push(c);
        let mut top = dom.get_element_node_mut(top).unwrap();
        top.append_child_text(&self.text_buf);
    }

    fn stack_contains(&self, dom: &Dom, names: &[&str]) -> bool {
        for &name in names {
            if let Some(name) = dom.find_str(name) {
                if self
                    .stack
                    .iter()
                    .find(|id| dom.get_element_node(**id).unwrap().name() == name)
                    .is_some()
                {
                    return true;
                }
            }
        }
        false
    }

    fn is_in_scope(&self, dom: &mut Dom, name: &str) -> bool {
        let name = dom.insert_str(name);
        self.is_index_in_scope(dom, name)
    }

    fn is_index_in_scope(&self, dom: &mut Dom, name: usize) -> bool {
        for &element in self.stack.iter().rev() {
            let element = dom.get_element_node(element).unwrap();
            if element.name() == name {
                return true;
            }
            if self.is_str_in(
                dom,
                element.name(),
                &["html", "table", "td", "th", "marquee"],
            ) {
                return false;
            }
        }
        unreachable!()
    }

    fn is_in_button_scope(&self, dom: &mut Dom, name: &str) -> bool {
        let name = dom.insert_str(name);
        for &element in self.stack.iter().rev() {
            let element = dom.get_element_node(element).unwrap();
            if element.name() == name {
                return true;
            }
            if self.is_str_in(
                dom,
                element.name(),
                &["html", "table", "td", "th", "marquee", "button"],
            ) {
                return false;
            }
        }
        unreachable!()
    }

    fn close_implied_end_elements(&mut self, dom: &Dom, names: &[&str]) {
        for &name in names {
            if let Some(name) = dom.find_str(name) {
                let top = *self.stack.last().unwrap();
                let top = dom.get_element_node(top).unwrap();
                if top.name() == name {
                    self.stack.pop();
                }
            }
        }
    }

    fn close_until(&mut self, dom: &Dom, name: usize) {
        while let Some(top) = self.stack.pop() {
            let top = dom.get_element_node(top).unwrap();
            if top.name() == name {
                break;
            }
        }
    }

    fn close_p(&mut self, dom: &Dom) {
        self.close_implied_end_elements(dom, &["dd", "dt", "li", "optgroup", "option"]);
        while let Some(top) = self.stack.pop() {
            let top = dom.get_element_node(top).unwrap();
            if self.is_str_in(dom, top.name(), &["p"]) {
                break;
            }
        }
    }

    fn stop_parsing(&mut self) -> Poll<ParseEvent> {
        self.stack.drain(..);
        Poll::Ready(ParseEvent::Done)
    }
}

impl<R: AsyncRead + Unpin> Parser<R> {
    pub fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        dom: &mut Dom,
    ) -> Poll<ParseEvent> {
        let this = self.get_mut();
        loop {
            let tok = {
                if let Some((_, tok)) = this.tok_buf.pop() {
                    Some(tok)
                } else {
                    match Pin::new(&mut this.tokenizer).poll_next(cx, dom) {
                        Poll::Ready(Some((loc, Ok(tok)))) => Some((loc, tok)),
                        Poll::Ready(Some((loc, Err(err)))) => {
                            return Poll::Ready(ParseEvent::Fatal(loc, err));
                        }
                        Poll::Ready(None) => None,
                        Poll::Pending => return Poll::Pending,
                    }
                    .map(|(_, tok)| tok)
                }
            };
            if this.skip_next_linefeed && matches!(tok, Some(Token::Char('\n'))) {
                this.skip_next_linefeed = false;
                continue;
            }
            loop {
                match this.insertion_mode {
                    InsertionMode::Initial => match tok {
                        Some(Token::Comment | Token::Char('\t' | '\n' | '\x0C' | ' ')) => break,
                        Some(Token::DocType) => todo!("doctype"),
                        _ => this.insertion_mode = InsertionMode::BeforeHtml,
                    },
                    InsertionMode::BeforeHtml => match tok {
                        Some(
                            Token::DocType
                            | Token::Comment
                            | Token::Char('\t' | '\n' | '\x0C' | ' '),
                        ) => break,
                        Some(Token::StartTag { name, attrs, .. })
                            if this.is_str_in(dom, name, &["html"]) =>
                        {
                            let mut root = dom.get_element_node_mut(ROOT_NODE_ID).unwrap();
                            let html = root.append_child_element(name, attrs);
                            this.stack.push(html);
                            this.insertion_mode = InsertionMode::BeforeHead;
                            break;
                        }
                        Some(Token::EndTag { name })
                            if !this.is_str_in(dom, name, &["head", "body", "html", "br"]) =>
                        {
                            break
                        }
                        _ => {
                            let html = dom.insert_str("html"); // synthetic
                            let mut root = dom.get_element_node_mut(ROOT_NODE_ID).unwrap();
                            let html = root.append_child_element(html, EMPTY_RANGE_INDEX);
                            this.stack.push(html);
                            this.insertion_mode = InsertionMode::BeforeHead;
                        }
                    },
                    InsertionMode::BeforeHead => match tok {
                        Some(
                            Token::DocType
                            | Token::Comment
                            | Token::Char('\t' | '\n' | '\x0C' | ' '),
                        ) => break,
                        Some(Token::StartTag { name, attrs, .. })
                            if this.is_str_in(dom, name, &["html"]) =>
                        {
                            if this.stack_contains(dom, &["template"]) {
                                break;
                            }
                            // error: merge attrs
                            let top = *this.stack.last().unwrap();
                            let mut top = dom.get_element_node_mut(top).unwrap();
                            top.insert_missing_attrs(attrs);
                            break;
                        }
                        Some(Token::StartTag { name, attrs, .. })
                            if this.is_str_in(dom, name, &["head"]) =>
                        {
                            let top = *this.stack.last().unwrap();
                            let mut top = dom.get_element_node_mut(top).unwrap();
                            let head = top.append_child_element(name, attrs);
                            this.head = Some(head);
                            this.stack.push(head);
                            this.insertion_mode = InsertionMode::InHead;
                            break;
                        }
                        Some(Token::EndTag { name })
                            if !this.is_str_in(dom, name, &["head", "body", "html", "br"]) =>
                        {
                            break
                        }
                        _ => {
                            let head = dom.insert_str("head"); // synthetic
                            let top = *this.stack.last().unwrap();
                            let mut top = dom.get_element_node_mut(top).unwrap();
                            let head = top.append_child_element(head, EMPTY_RANGE_INDEX);
                            this.head = Some(head);
                            this.stack.push(head);
                            this.insertion_mode = InsertionMode::InHead;
                        }
                    },
                    InsertionMode::InHead => match tok {
                        Some(Token::Char(c)) if "\t\n\x0C ".contains(c) => {
                            this.append_text(dom, c);
                            break;
                        }
                        Some(Token::DocType | Token::Comment) => break,
                        Some(Token::StartTag { name, attrs, .. })
                            if this.is_str_in(dom, name, &["html"]) =>
                        {
                            if this.stack_contains(dom, &["template"]) {
                                break;
                            }
                            // error: merge attrs
                            let top = *this.stack.last().unwrap();
                            let mut top = dom.get_element_node_mut(top).unwrap();
                            top.insert_missing_attrs(attrs);
                            break;
                        }
                        Some(Token::StartTag { name, .. })
                            if this.is_str_in(dom, name, &["base", "link", "meta"]) =>
                        {
                            todo!()
                        }
                        Some(Token::StartTag { name, attrs, .. })
                            if this.is_str_in(dom, name, &["title"]) =>
                        {
                            let top = *this.stack.last().unwrap();
                            let mut top = dom.get_element_node_mut(top).unwrap();
                            let element = top.append_child_element(name, attrs);
                            this.stack.push(element);
                            this.tokenizer.set_state(State::RcData);
                            this.original_insertion_mode = this.insertion_mode;
                            this.insertion_mode = InsertionMode::Text;
                            break;
                        }
                        Some(Token::StartTag { name, .. })
                            if this.is_str_in(dom, name, &["noframes", "style"]) =>
                        {
                            todo!()
                        }
                        Some(Token::StartTag { name, .. })
                            if this.is_str_in(dom, name, &["noscript"]) =>
                        {
                            todo!()
                        }
                        Some(Token::StartTag { name, .. })
                            if this.is_str_in(dom, name, &["script"]) =>
                        {
                            todo!()
                        }
                        Some(Token::EndTag { name }) if this.is_str_in(dom, name, &["head"]) => {
                            this.stack.pop();
                            this.insertion_mode = InsertionMode::AfterHead;
                            break;
                        }
                        Some(Token::EndTag { name })
                            if this.is_str_in(dom, name, &["body", "html", "br"]) =>
                        {
                            this.stack.pop();
                            this.insertion_mode = InsertionMode::AfterHead;
                        }
                        Some(Token::StartTag { name, .. })
                            if this.is_str_in(dom, name, &["template"]) =>
                        {
                            todo!()
                        }
                        Some(Token::EndTag { name })
                            if this.is_str_in(dom, name, &["template"]) =>
                        {
                            todo!()
                        }
                        Some(Token::StartTag { name, .. })
                            if this.is_str_in(dom, name, &["head"]) =>
                        {
                            break
                        }
                        Some(Token::EndTag { .. }) => break,
                        _ => {
                            this.stack.pop();
                            this.insertion_mode = InsertionMode::AfterHead;
                        }
                    },
                    InsertionMode::AfterHead => match tok {
                        Some(Token::Char(c)) if "\t\n\x0C ".contains(c) => {
                            this.append_text(dom, c);
                            break;
                        }
                        Some(Token::DocType | Token::Comment) => break,
                        Some(Token::StartTag { name, attrs, .. })
                            if this.is_str_in(dom, name, &["html"]) =>
                        {
                            if this.stack_contains(dom, &["template"]) {
                                break;
                            }
                            // error: merge attrs
                            let top = *this.stack.last().unwrap();
                            let mut top = dom.get_element_node_mut(top).unwrap();
                            top.insert_missing_attrs(attrs);
                            break;
                        }
                        Some(Token::StartTag { name, attrs, .. })
                            if this.is_str_in(dom, name, &["body"]) =>
                        {
                            let top = *this.stack.last().unwrap();
                            let mut top = dom.get_element_node_mut(top).unwrap();
                            let body = top.append_child_element(name, attrs);
                            this.stack.push(body);
                            this.insertion_mode = InsertionMode::InBody;
                            break;
                        }
                        Some(Token::StartTag { name, .. })
                            if this.is_str_in(
                                dom,
                                name,
                                &[
                                    "base", "link", "meta", "noframes", "script", "style",
                                    "template", "title",
                                ],
                            ) =>
                        {
                            todo!()
                        }
                        Some(Token::EndTag { name })
                            if this.is_str_in(dom, name, &["template"]) =>
                        {
                            todo!()
                        }
                        Some(Token::EndTag { name })
                            if this.is_str_in(dom, name, &["body", "html", "br"]) =>
                        {
                            let body = dom.insert_str("body"); // synthetic
                            let top = *this.stack.last().unwrap();
                            let mut top = dom.get_element_node_mut(top).unwrap();
                            let body = top.append_child_element(body, EMPTY_RANGE_INDEX);
                            this.stack.push(body);
                            this.insertion_mode = InsertionMode::InBody;
                        }
                        Some(Token::StartTag { name, .. })
                            if this.is_str_in(dom, name, &["head"]) =>
                        {
                            break
                        }
                        Some(Token::EndTag { .. }) => break,
                        _ => {
                            let body = dom.insert_str("body"); // synthetic
                            let top = *this.stack.last().unwrap();
                            let mut top = dom.get_element_node_mut(top).unwrap();
                            let body = top.append_child_element(body, EMPTY_RANGE_INDEX);
                            this.stack.push(body);
                            this.insertion_mode = InsertionMode::InBody;
                        }
                    },
                    InsertionMode::InBody => match tok {
                        Some(Token::Char(c)) if "\t\n\x0C ".contains(c) => {
                            this.append_text(dom, c);
                            break;
                        }
                        Some(Token::Char(c)) => {
                            this.append_text(dom, c);
                            this.frameset_ok = false;
                            break;
                        }
                        Some(Token::DocType | Token::Comment) => break,
                        Some(Token::StartTag { name, attrs, .. })
                            if this.is_str_in(dom, name, &["html"]) =>
                        {
                            if this.stack_contains(dom, &["template"]) {
                                break;
                            }
                            // error: merge attrs
                            let top = *this.stack.last().unwrap();
                            let mut top = dom.get_element_node_mut(top).unwrap();
                            top.insert_missing_attrs(attrs);
                            break;
                        }
                        Some(Token::StartTag { name, .. })
                            if this.is_str_in(
                                dom,
                                name,
                                &[
                                    "base", "link", "meta", "noframes", "script", "style",
                                    "template", "title",
                                ],
                            ) =>
                        {
                            todo!()
                        }
                        Some(Token::EndTag { name })
                            if this.is_str_in(dom, name, &["template"]) =>
                        {
                            todo!()
                        }
                        Some(Token::StartTag { name, attrs, .. })
                            if this.is_str_in(dom, name, &["body"]) =>
                        {
                            if (this.stack.len() == 1) || this.stack_contains(dom, &["template"]) {
                                break;
                            }
                            let element = this.stack[1];
                            let mut element = dom.get_element_node_mut(element).unwrap();
                            if element.name() != name {
                                break;
                            }
                            this.frameset_ok = false;
                            // error: merge attrs
                            element.insert_missing_attrs(attrs);
                            break;
                        }
                        Some(Token::StartTag { name, .. })
                            if this.is_str_in(dom, name, &["frameset"]) =>
                        {
                            todo!()
                        }
                        None => {
                            if !this.template_insertion_modes.is_empty() {
                                todo!();
                            }
                            return this.stop_parsing();
                        }
                        Some(Token::EndTag { name }) if this.is_str_in(dom, name, &["body"]) => {
                            if !this.is_in_scope(dom, "body") {
                                break;
                            }
                            this.insertion_mode = InsertionMode::AfterBody;
                            break;
                        }
                        Some(Token::EndTag { name }) if this.is_str_in(dom, name, &["html"]) => {
                            if !this.is_in_scope(dom, "body") {
                                break;
                            }
                            this.insertion_mode = InsertionMode::AfterBody;
                        }
                        Some(Token::StartTag { name, attrs, .. })
                            if this.is_str_in(
                                dom,
                                name,
                                &[
                                    "address",
                                    "article",
                                    "aside",
                                    "blockquote",
                                    "center",
                                    "details",
                                    "dialog",
                                    "dir",
                                    "div",
                                    "dl",
                                    "fieldset",
                                    "figcaption",
                                    "figure",
                                    "footer",
                                    "header",
                                    "hgroup",
                                    "main",
                                    "menu",
                                    "nav",
                                    "ol",
                                    "p",
                                    "search",
                                    "section",
                                    "summary",
                                    "ul",
                                ],
                            ) =>
                        {
                            if this.is_in_button_scope(dom, "p") {
                                this.close_p(dom);
                            }
                            let top = *this.stack.last().unwrap();
                            let mut top = dom.get_element_node_mut(top).unwrap();
                            let element = top.append_child_element(name, attrs);
                            this.stack.push(element);
                            break;
                        }
                        Some(Token::StartTag { name, attrs, .. })
                            if this.is_str_in(dom, name, &["h1", "h2", "h3", "h4", "h5", "h6"]) =>
                        {
                            if this.is_in_button_scope(dom, "p") {
                                this.close_p(dom);
                            }
                            let mut top = *this.stack.last().unwrap();
                            let element = dom.get_element_node(top).unwrap();
                            if this.is_str_in(
                                dom,
                                element.name(),
                                &["h1", "h2", "h3", "h4", "h5", "h6"],
                            ) {
                                this.stack.pop();
                                top = *this.stack.last().unwrap();
                            }
                            let mut top = dom.get_element_node_mut(top).unwrap();
                            let element = top.append_child_element(name, attrs);
                            this.stack.push(element);
                            break;
                        }
                        Some(Token::StartTag { name, attrs, .. })
                            if this.is_str_in(dom, name, &["pre", "listing"]) =>
                        {
                            if this.is_in_button_scope(dom, "p") {
                                this.close_p(dom);
                            }
                            let top = *this.stack.last().unwrap();
                            let mut top = dom.get_element_node_mut(top).unwrap();
                            let element = top.append_child_element(name, attrs);
                            this.stack.push(element);
                            this.frameset_ok = false;
                            this.skip_next_linefeed = true;
                            break;
                        }
                        Some(Token::StartTag { name, .. })
                            if this.is_str_in(dom, name, &["form"]) =>
                        {
                            todo!()
                        }
                        Some(Token::StartTag { name, .. })
                            if this.is_str_in(dom, name, &["li"]) =>
                        {
                            todo!()
                        }
                        Some(Token::StartTag { name, .. })
                            if this.is_str_in(dom, name, &["dd", "dt"]) =>
                        {
                            todo!()
                        }
                        Some(Token::StartTag { name, .. })
                            if this.is_str_in(dom, name, &["plaintext"]) =>
                        {
                            todo!()
                        }
                        Some(Token::StartTag { name, .. })
                            if this.is_str_in(dom, name, &["button"]) =>
                        {
                            todo!()
                        }
                        Some(Token::EndTag { name })
                            if this.is_str_in(
                                dom,
                                name,
                                &[
                                    "address",
                                    "article",
                                    "aside",
                                    "blockquote",
                                    "center",
                                    "details",
                                    "dialog",
                                    "dir",
                                    "div",
                                    "dl",
                                    "fieldset",
                                    "figcaption",
                                    "figure",
                                    "footer",
                                    "header",
                                    "hgroup",
                                    "main",
                                    "menu",
                                    "nav",
                                    "ol",
                                    "p",
                                    "search",
                                    "section",
                                    "summary",
                                    "ul",
                                ],
                            ) =>
                        {
                            if !this.is_index_in_scope(dom, name) {
                                break;
                            }
                            this.close_implied_end_elements(
                                dom,
                                &["dd", "dt", "li", "optgroup", "option", "p"],
                            );
                            this.close_until(dom, name);
                            break;
                        }
                        Some(Token::EndTag { name }) if this.is_str_in(dom, name, &["form"]) => {
                            todo!()
                        }
                        Some(Token::EndTag { name }) if this.is_str_in(dom, name, &["p"]) => {
                            todo!()
                        }
                        Some(Token::EndTag { name }) if this.is_str_in(dom, name, &["li"]) => {
                            todo!()
                        }
                        Some(Token::EndTag { name })
                            if this.is_str_in(dom, name, &["dd", "dt"]) =>
                        {
                            todo!()
                        }
                        Some(Token::EndTag { name })
                            if this.is_str_in(dom, name, &["h1", "h2", "h3", "h4", "h5", "h6"]) =>
                        {
                            todo!()
                        }
                        Some(Token::StartTag { name, .. }) if this.is_str_in(dom, name, &["a"]) => {
                            todo!()
                        }
                        Some(Token::StartTag { name, .. })
                            if this.is_str_in(
                                dom,
                                name,
                                &[
                                    "b", "big", "code", "em", "font", "i", "s", "small", "strike",
                                    "strong", "tt", "u",
                                ],
                            ) =>
                        {
                            todo!()
                        }
                        Some(Token::StartTag { name, .. })
                            if this.is_str_in(dom, name, &["nobr"]) =>
                        {
                            todo!()
                        }
                        Some(Token::EndTag { name })
                            if this.is_str_in(
                                dom,
                                name,
                                &[
                                    "a", "b", "big", "code", "em", "font", "i", "nobr", "s",
                                    "small", "strike", "strong", "tt", "u",
                                ],
                            ) =>
                        {
                            todo!()
                        }
                        Some(Token::StartTag { name, .. })
                            if this.is_str_in(dom, name, &["applet", "marquee", "object"]) =>
                        {
                            todo!()
                        }
                        Some(Token::EndTag { name })
                            if this.is_str_in(dom, name, &["applet", "marquee", "object"]) =>
                        {
                            todo!()
                        }
                        Some(Token::StartTag { name, .. })
                            if this.is_str_in(dom, name, &["table"]) =>
                        {
                            todo!()
                        }
                        Some(Token::EndTag { name }) if this.is_str_in(dom, name, &["br"]) => {
                            todo!()
                        }
                        Some(Token::StartTag { name, .. })
                            if this.is_str_in(
                                dom,
                                name,
                                &["area", "br", "embed", "img", "keygen", "wbr"],
                            ) =>
                        {
                            todo!()
                        }
                        Some(Token::StartTag { name, .. })
                            if this.is_str_in(dom, name, &["input"]) =>
                        {
                            todo!()
                        }
                        Some(Token::StartTag { name, .. })
                            if this.is_str_in(dom, name, &["param", "source", "track"]) =>
                        {
                            todo!()
                        }
                        Some(Token::StartTag { name, .. })
                            if this.is_str_in(dom, name, &["hr"]) =>
                        {
                            todo!()
                        }
                        Some(Token::StartTag { name, .. })
                            if this.is_str_in(dom, name, &["image"]) =>
                        {
                            todo!()
                        }
                        Some(Token::StartTag { name, .. })
                            if this.is_str_in(dom, name, &["textarea"]) =>
                        {
                            todo!()
                        }
                        Some(Token::StartTag { name, .. })
                            if this.is_str_in(dom, name, &["iframe"]) =>
                        {
                            todo!()
                        }
                        Some(Token::StartTag { name, .. })
                            if this.is_str_in(dom, name, &["noembed"]) =>
                        {
                            todo!()
                        }
                        Some(Token::StartTag { name, .. })
                            if this.is_str_in(dom, name, &["noscript"]) =>
                        {
                            todo!()
                        }
                        Some(Token::StartTag { name, .. })
                            if this.is_str_in(dom, name, &["select"]) =>
                        {
                            todo!()
                        }
                        Some(Token::StartTag { name, .. })
                            if this.is_str_in(dom, name, &["optgroup", "option"]) =>
                        {
                            todo!()
                        }
                        Some(Token::StartTag { name, .. })
                            if this.is_str_in(
                                dom,
                                name,
                                &[
                                    "caption", "col", "colgroup", "frame", "head", "tbody", "td",
                                    "tfoot", "th", "thead", "tr",
                                ],
                            ) =>
                        {
                            break
                        }
                        Some(Token::StartTag { name, attrs, .. }) => {
                            let top = *this.stack.last().unwrap();
                            let mut top = dom.get_element_node_mut(top).unwrap();
                            let element = top.append_child_element(name, attrs);
                            this.stack.push(element);
                            break;
                        }
                        Some(Token::EndTag { .. }) => {
                            todo!()
                        }
                    },
                    InsertionMode::AfterBody => match tok {
                        Some(Token::Char(c)) if "\t\n\x0C ".contains(c) => {
                            this.append_text(dom, c);
                            break;
                        }
                        Some(Token::DocType | Token::Comment) => break,
                        Some(Token::StartTag { name, attrs, .. })
                            if this.is_str_in(dom, name, &["html"]) =>
                        {
                            if this.stack_contains(dom, &["template"]) {
                                break;
                            }
                            // error: merge attrs
                            let top = *this.stack.last().unwrap();
                            let mut top = dom.get_element_node_mut(top).unwrap();
                            top.insert_missing_attrs(attrs);
                            break;
                        }
                        Some(Token::EndTag { name }) if this.is_str_in(dom, name, &["html"]) => {
                            this.insertion_mode = InsertionMode::AfterAfterBody;
                            break;
                        }
                        None => return this.stop_parsing(),
                        _ => this.insertion_mode = InsertionMode::InBody,
                    },
                    InsertionMode::AfterAfterBody => match tok {
                        Some(Token::Char(c)) if "\t\n\x0C ".contains(c) => {
                            this.append_text(dom, c);
                            break;
                        }
                        Some(Token::DocType | Token::Comment) => break,
                        Some(Token::StartTag { name, attrs, .. })
                            if this.is_str_in(dom, name, &["html"]) =>
                        {
                            if this.stack_contains(dom, &["template"]) {
                                break;
                            }
                            // error: merge attrs
                            let top = *this.stack.last().unwrap();
                            let mut top = dom.get_element_node_mut(top).unwrap();
                            top.insert_missing_attrs(attrs);
                            break;
                        }
                        None => return this.stop_parsing(),
                        _ => this.insertion_mode = InsertionMode::InBody,
                    },
                    InsertionMode::Text => match tok {
                        Some(Token::Char(c)) => {
                            this.append_text(dom, c);
                            break;
                        }
                        None => {
                            let top = this.stack.pop().unwrap();
                            this.insertion_mode = this.original_insertion_mode;
                            let element = dom.get_element_node(top).unwrap();
                            if this.is_str_in(dom, element.name(), &["title"]) {
                                return Poll::Ready(ParseEvent::Title(top));
                            }
                            if this.is_str_in(dom, element.name(), &["style"]) {
                                return Poll::Ready(ParseEvent::Style(top));
                            }
                        }
                        Some(Token::EndTag { name }) if this.is_str_in(dom, name, &["script"]) => {
                            this.stack.pop();
                            this.insertion_mode = this.original_insertion_mode;
                        }
                        Some(Token::EndTag { .. }) => {
                            let top = this.stack.pop().unwrap();
                            this.insertion_mode = this.original_insertion_mode;
                            let element = dom.get_element_node(top).unwrap();
                            if this.is_str_in(dom, element.name(), &["title"]) {
                                return Poll::Ready(ParseEvent::Title(top));
                            }
                            if this.is_str_in(dom, element.name(), &["style"]) {
                                return Poll::Ready(ParseEvent::Style(top));
                            }
                        }
                        _ => unreachable!(),
                    },
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use smol::io::Cursor;

    use super::*;
    use crate::asyncro;

    fn cx<'a>() -> Context<'a> {
        Context::from_waker(asyncro::noop_waker_ref())
    }

    fn assert_done<R: AsyncRead + Unpin>(
        cx: &mut Context<'_>,
        parser: &mut Parser<R>,
        dom: &mut Dom,
    ) {
        assert!(matches!(
            Pin::new(parser).poll_next(cx, dom),
            Poll::Ready(ParseEvent::Done)
        ));
    }

    fn assert_title<R: AsyncRead + Unpin>(
        cx: &mut Context<'_>,
        parser: &mut Parser<R>,
        dom: &mut Dom,
        title: &str,
    ) {
        let poll = Pin::new(parser).poll_next(cx, dom);
        assert!(matches!(poll, Poll::Ready(ParseEvent::Title(_))));
        if let Poll::Ready(ParseEvent::Title(id)) = poll {
            let node = dom.get_element_node(id).unwrap();
            let child = node.child_indices().start;
            let text = dom.get_node_id_by_index(child).unwrap();
            let text = dom.get_text_node(text).unwrap();
            assert_eq!(title, text.text());
        }
    }

    fn assert_dom(dom: &Dom, repr: &str) {
        let mut tree = io::Cursor::new(Vec::new());
        dom.write_tree(&mut tree).unwrap();
        let string = String::from_utf8(tree.into_inner()).unwrap();
        assert_eq!(repr.trim_start(), string);
    }

    #[test]
    fn empty() {
        let reader = Cursor::new("");
        let reader = AsyncStrReader::new(reader);
        let mut dom = Dom::new();
        let mut cx = cx();
        let mut parser = Parser::new(reader);
        assert_done(&mut cx, &mut parser, &mut dom);
        assert_dom(
            &dom,
            r#"
<>
  <html>
    <head>
    <body>
"#,
        );
    }

    #[test]
    fn text() {
        let reader = Cursor::new("test");
        let reader = AsyncStrReader::new(reader);
        let mut dom = Dom::new();
        let mut cx = cx();
        let mut parser = Parser::new(reader);
        assert_done(&mut cx, &mut parser, &mut dom);
        assert_dom(
            &dom,
            r#"
<>
  <html>
    <head>
    <body>
      <>test
"#,
        );
    }

    #[test]
    fn title() {
        let reader = Cursor::new("<title>test</title>");
        let reader = AsyncStrReader::new(reader);
        let mut dom = Dom::new();
        let mut cx = cx();
        let mut parser = Parser::new(reader);
        assert_title(&mut cx, &mut parser, &mut dom, "test");
        assert_done(&mut cx, &mut parser, &mut dom);
        assert_dom(
            &dom,
            r#"
<>
  <html>
    <head>
      <title>
        <>test
    <body>
"#,
        );
    }
}
