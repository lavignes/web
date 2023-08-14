use std::{
    char,
    io::{self, Read},
    str,
};

use crate::dom::{Dom, EMPTY_RANGE_INDEX, ROOT_NODE_ID};

#[cfg(test)]
mod tests;

#[derive(thiserror::Error, Debug, Clone)]
#[error("{msg}")]
struct HtmlError {
    msg: String,
}

impl From<&str> for HtmlError {
    fn from(value: &str) -> Self {
        Self {
            msg: value.to_string(),
        }
    }
}

impl From<String> for HtmlError {
    fn from(msg: String) -> Self {
        Self { msg }
    }
}

impl From<CharReaderError> for HtmlError {
    fn from(err: CharReaderError) -> Self {
        Self {
            msg: err.to_string(),
        }
    }
}

#[derive(Copy, Clone)]
struct Position(usize, usize);

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

struct Html<R> {
    lexer: Lexer<R>,
    insertion_mode: InsertionMode,
    stash: Result<Token, HtmlError>,
    open_elements: Vec<usize>,
    head_element: Option<usize>,
    form_element: Option<usize>,
    text: String,
}

impl<R: Read> Html<R> {
    fn new(reader: R, dom: Dom) -> Self {
        let mut lexer = Lexer::new(reader, dom);
        let next = lexer.next();
        let stash = next.map(|_| lexer.tok);
        Self {
            lexer,
            insertion_mode: InsertionMode::Initial,
            stash,
            open_elements: Vec::new(),
            head_element: None,
            form_element: None,
            text: String::new(),
        }
    }

    fn peek(&mut self) -> Result<Token, HtmlError> {
        self.stash.clone()
    }

    fn next(&mut self) -> Result<Token, HtmlError> {
        let stash = self.stash.clone();
        let next = self.lexer.next();
        self.stash = next.map(|_| self.lexer.tok);
        stash
    }

    fn node_name_in(&self, index: usize, names: &[&str]) -> bool {
        for &name in names {
            let name = self.lexer.dom.find_text(name);
            if matches!(name, Some(i) if i == index) {
                return true;
            }
        }
        false
    }

    fn append_text(&mut self, c: char) {
        self.lexer.dom.append_char(c);
        let element = self
            .lexer
            .dom
            .get_element_node(*self.open_elements.last().unwrap())
            .unwrap();
        // if the last child is a text node, then we'll update its text
        if let Some(child) = element.child_indicies().last() {
            if let Some(child) = self.lexer.dom.get_node_id_by_index(child) {
                if let Some(mut text) = self.lexer.dom.get_text_node_mut(child) {
                    self.text.push(c);
                    text.set_text(&self.text);
                    return;
                }
            }
        }
        // otherwise create a new child to store the text
        self.text.clear();
        self.text.push(c);
        self.lexer
            .dom
            .get_element_node_mut(*self.open_elements.last().unwrap())
            .unwrap()
            .append_child_text(&self.text);
    }

    fn is_node_in_scope(&mut self, name: usize) -> bool {
        for &node in self.open_elements.iter().rev() {
            let node = self.lexer.dom.get_element_node(node).unwrap();
            if node.name() == name {
                return true;
            }
            if self.node_name_in(node.name(), &["html", "table", "td", "th", "marquee"]) {
                return false;
            }
        }
        unreachable!()
    }

    fn is_node_in_button_scope(&mut self, name: usize) -> bool {
        for &node in self.open_elements.iter().rev() {
            let node = self.lexer.dom.get_element_node(node).unwrap();
            if node.name() == name {
                return true;
            }
            if self.node_name_in(
                node.name(),
                &["html", "table", "td", "th", "marquee", "button"],
            ) {
                return false;
            }
        }
        unreachable!()
    }

    fn close_implied_end_tags(&mut self, names: &[&str]) {
        for &name in names {
            let name = self.lexer.dom.find_text(name);
            if let Some(name) = name {
                let node = self
                    .lexer
                    .dom
                    .get_element_node(*self.open_elements.last().unwrap())
                    .unwrap();
                if node.name() == name {
                    self.open_elements.pop();
                }
            }
        }
    }

    fn close_p_node(&mut self) {
        self.close_implied_end_tags(&["dd", "dt", "li", "optgroup", "option"]);
        while let Some(node) = self.open_elements.pop() {
            let node = self.lexer.dom.get_element_node(node).unwrap();
            if self.node_name_in(node.name(), &["p"]) {
                break;
            }
        }
    }

    fn stop_parsing(&mut self) -> Result<(), HtmlError> {
        self.open_elements.drain(..);
        Ok(())
    }

    fn into_dom(self) -> Dom {
        let Self { lexer, .. } = self;
        lexer.dom
    }

    fn parse(&mut self) -> Result<(), HtmlError> {
        loop {
            match self.insertion_mode {
                InsertionMode::Initial => match self.peek()? {
                    Token::Comment | Token::Char('\t' | '\n' | '\x0C' | ' ') => {
                        self.next()?;
                    }
                    Token::DocType => todo!("doctype"),
                    _ => {
                        self.insertion_mode = InsertionMode::BeforeHtml;
                    }
                },
                InsertionMode::BeforeHtml => match self.peek()? {
                    Token::DocType | Token::Comment | Token::Char('\t' | '\n' | '\x0C' | ' ') => {
                        self.next()?;
                    }
                    Token::Tag {
                        end: false, name, ..
                    } if self.node_name_in(name, &["html"]) => {
                        self.insertion_mode = InsertionMode::BeforeHead;
                        let element = {
                            let mut root =
                                self.lexer.dom.get_element_node_mut(ROOT_NODE_ID).unwrap();
                            root.append_child_element(name, &self.lexer.attrs)
                        };
                        self.open_elements.push(element);
                        self.next()?;
                    }
                    Token::Tag {
                        end: true, name, ..
                    } if !self.node_name_in(name, &["head", "body", "html", "br"]) => {
                        self.next()?;
                    }
                    _ => {
                        self.insertion_mode = InsertionMode::BeforeHead;
                        // otherwise, create a fake html element
                        let html = self.lexer.dom.insert_text("html");
                        let element = {
                            let mut root =
                                self.lexer.dom.get_element_node_mut(ROOT_NODE_ID).unwrap();
                            root.append_child_element(html, &[])
                        };
                        self.open_elements.push(element);
                    }
                },
                InsertionMode::BeforeHead => match self.peek()? {
                    Token::Char('\t' | '\n' | '\x0C' | ' ') => {
                        self.next()?;
                    }
                    Token::DocType | Token::Comment => {
                        self.next()?;
                    }
                    Token::Tag {
                        end: false, name, ..
                    } if self.node_name_in(name, &["head"]) => {
                        self.insertion_mode = InsertionMode::InHead;
                        let element = {
                            let mut root = self
                                .lexer
                                .dom
                                .get_element_node_mut(*self.open_elements.last().unwrap())
                                .unwrap();
                            root.append_child_element(name, &self.lexer.attrs)
                        };
                        self.head_element = Some(element);
                        self.open_elements.push(element);
                        self.next()?;
                    }
                    Token::Tag {
                        end: true, name, ..
                    } if !self.node_name_in(name, &["head", "body", "html", "br"]) => {
                        self.next()?;
                    }
                    _ => {
                        self.insertion_mode = InsertionMode::InHead;
                        // otherwise, create a fake head element
                        let head = self.lexer.dom.insert_text("head");
                        let element = {
                            let mut root = self
                                .lexer
                                .dom
                                .get_element_node_mut(*self.open_elements.last().unwrap())
                                .unwrap();
                            root.append_child_element(head, &[])
                        };
                        self.head_element = Some(head);
                        self.open_elements.push(element);
                    }
                },
                InsertionMode::InHead => match self.peek()? {
                    Token::Char(c @ '\t' | c @ '\n' | c @ '\x0C' | c @ ' ') => {
                        self.append_text(c);
                        self.next()?;
                    }
                    Token::DocType | Token::Comment => {
                        self.next()?;
                    }
                    Token::Tag {
                        end: false, name, ..
                    } if self.node_name_in(name, &["html"]) => {
                        todo!("html in head")
                    }
                    Token::Tag {
                        self_closing,
                        end: false,
                        name,
                    } if self.node_name_in(name, &["link"]) => {
                        todo!("link in head")
                    }
                    Token::Tag {
                        end: false, name, ..
                    } if self.node_name_in(name, &["title"]) => {
                        todo!("title")
                    }
                    Token::Tag {
                        end: false, name, ..
                    } if self.node_name_in(name, &["style"]) => {
                        todo!("style")
                    }
                    Token::Tag {
                        end: true, name, ..
                    } if self.node_name_in(name, &["head"]) => {
                        self.insertion_mode = InsertionMode::AfterHead;
                        self.open_elements.pop();
                        self.next()?;
                    }
                    Token::Tag {
                        end: false, name, ..
                    } if self.node_name_in(name, &["head"]) => {
                        self.next()?;
                    }
                    Token::Tag { end: true, .. } => {
                        self.next()?;
                    }
                    _ => {
                        self.insertion_mode = InsertionMode::AfterHead;
                        self.open_elements.pop();
                    }
                },
                InsertionMode::AfterHead => match self.peek()? {
                    Token::Char(c @ '\t' | c @ '\n' | c @ '\x0C' | c @ ' ') => {
                        self.append_text(c);
                        self.next()?;
                    }
                    Token::DocType | Token::Comment => {
                        self.next()?;
                    }
                    Token::Tag {
                        end: false, name, ..
                    } if self.node_name_in(name, &["html"]) => {
                        todo!("html in body")
                    }
                    Token::Tag {
                        end: false, name, ..
                    } if self.node_name_in(name, &["body"]) => {
                        self.insertion_mode = InsertionMode::InBody;
                        let element = {
                            let mut root = self
                                .lexer
                                .dom
                                .get_element_node_mut(*self.open_elements.last().unwrap())
                                .unwrap();
                            root.append_child_element(name, &self.lexer.attrs)
                        };
                        self.open_elements.push(element);
                        self.next()?;
                    }
                    Token::Tag {
                        end: false, name, ..
                    } if self.node_name_in(name, &["link", "style", "title"]) => {
                        todo!("head element in body")
                    }
                    Token::Tag {
                        end: false, name, ..
                    } if self.node_name_in(name, &["head"]) => {
                        self.next()?;
                    }
                    Token::Tag {
                        end: true, name, ..
                    } if self.node_name_in(name, &["body", "html", "br"]) => {
                        self.insertion_mode = InsertionMode::InBody;
                        // otherwise, create a fake body element
                        let body = self.lexer.dom.insert_text("body");
                        let element = {
                            let mut root = self
                                .lexer
                                .dom
                                .get_element_node_mut(*self.open_elements.last().unwrap())
                                .unwrap();
                            root.append_child_element(body, &[])
                        };
                        self.head_element = Some(body);
                        self.open_elements.push(element);
                    }
                    Token::Tag { end: true, .. } => {
                        self.next()?;
                    }
                    _ => {
                        self.insertion_mode = InsertionMode::InBody;
                        // otherwise, create a fake body element
                        let body = self.lexer.dom.insert_text("body");
                        let element = {
                            let mut root = self
                                .lexer
                                .dom
                                .get_element_node_mut(*self.open_elements.last().unwrap())
                                .unwrap();
                            root.append_child_element(body, &[])
                        };
                        self.head_element = Some(body);
                        self.open_elements.push(element);
                    }
                },
                InsertionMode::InBody => match self.peek()? {
                    Token::Char(c) => {
                        // TODO: reconstruct active formatting elements?
                        self.append_text(c);
                        self.next()?;
                    }
                    Token::DocType | Token::Comment => {
                        self.next()?;
                    }
                    Token::Tag {
                        end: false, name, ..
                    } if self.node_name_in(name, &["html"]) => {
                        todo!("html in body")
                    }
                    Token::Tag {
                        end: false, name, ..
                    } if self.node_name_in(name, &["link", "style", "title"]) => {
                        todo!("head element in body")
                    }
                    Token::Tag {
                        end: false, name, ..
                    } if self.node_name_in(name, &["body"]) => {
                        if self.open_elements.len() == 1 {
                            self.next()?;
                            continue;
                        }
                        let next_to_last_element = self
                            .lexer
                            .dom
                            .get_element_node(self.open_elements[self.open_elements.len() - 2])
                            .unwrap();
                        if self.node_name_in(next_to_last_element.name(), &["html"]) {
                            self.next()?;
                        }
                    }
                    Token::Tag {
                        end: false, name, ..
                    } if self.node_name_in(name, &["body"]) => {
                        todo!("body in body")
                    }
                    Token::Tag {
                        end: false, name, ..
                    } if self.node_name_in(name, &["link", "style", "title"]) => {
                        todo!("head element in body")
                    }
                    Token::Eof => {
                        return self.stop_parsing();
                    }
                    Token::Tag {
                        end: true, name, ..
                    } if self.node_name_in(name, &["body"]) => {
                        let body = self.lexer.dom.insert_text("body");
                        if !self.is_node_in_scope(body) {
                            self.next()?;
                            continue;
                        }
                        self.insertion_mode = InsertionMode::AfterBody;
                        self.next()?;
                    }
                    Token::Tag {
                        end: true, name, ..
                    } if self.node_name_in(name, &["html"]) => {
                        let body = self.lexer.dom.insert_text("body");
                        if !self.is_node_in_scope(body) {
                            self.next()?;
                            continue;
                        }
                        self.insertion_mode = InsertionMode::AfterBody;
                    }
                    Token::Tag {
                        end: false, name, ..
                    } if self.node_name_in(
                        name,
                        &[
                            "address",
                            "article",
                            "aside",
                            "blockquote",
                            "center",
                            "details",
                            "dialog",
                            "div",
                            "dl",
                            "fieldset",
                            "figcaption",
                            "figure",
                            "footer",
                            "header",
                            "nav",
                            "ol",
                            "p",
                            "section",
                            "summary",
                            "ul",
                        ],
                    ) =>
                    {
                        let p = self.lexer.dom.insert_text("p");
                        if self.is_node_in_button_scope(p) {
                            self.close_p_node();
                        }
                        let mut node = self
                            .lexer
                            .dom
                            .get_element_node_mut(*self.open_elements.last().unwrap())
                            .unwrap();
                        let node = node.append_child_element(name, &self.lexer.attrs);
                        self.open_elements.push(node);
                        self.next()?;
                    }
                    Token::Tag {
                        end: false, name, ..
                    } if self.node_name_in(name, &["h1", "h2", "h3", "h4", "h5", "h6"]) => {
                        let p = self.lexer.dom.insert_text("p");
                        if self.is_node_in_button_scope(p) {
                            self.close_p_node();
                        }
                        let current = self
                            .lexer
                            .dom
                            .get_element_node(*self.open_elements.last().unwrap())
                            .unwrap();
                        if current.name() == name {
                            self.open_elements.pop();
                        }
                        let mut node = self
                            .lexer
                            .dom
                            .get_element_node_mut(*self.open_elements.last().unwrap())
                            .unwrap();
                        let node = node.append_child_element(name, &self.lexer.attrs);
                        self.open_elements.push(node);
                        self.next()?;
                    }
                    Token::Tag {
                        end: false, name, ..
                    } if self.node_name_in(name, &["pre"]) => {
                        let p = self.lexer.dom.insert_text("p");
                        if self.is_node_in_button_scope(p) {
                            self.close_p_node();
                        }
                        let mut node = self
                            .lexer
                            .dom
                            .get_element_node_mut(*self.open_elements.last().unwrap())
                            .unwrap();
                        let node = node.append_child_element(name, &self.lexer.attrs);
                        self.open_elements.push(node);
                        self.next()?;
                        if let Token::Char('\n') = self.peek()? {
                            self.next()?;
                        }
                    }
                    Token::Tag {
                        end: false, name, ..
                    } if self.node_name_in(name, &["form"]) => {
                        if self.form_element.is_some() {
                            self.next()?;
                            continue;
                        }
                        let p = self.lexer.dom.insert_text("p");
                        if !self.is_node_in_button_scope(p) {
                            self.close_p_node();
                        }
                        let mut node = self
                            .lexer
                            .dom
                            .get_element_node_mut(*self.open_elements.last().unwrap())
                            .unwrap();
                        let node = node.append_child_element(name, &self.lexer.attrs);
                        self.open_elements.push(node);
                        self.form_element = Some(node);
                        self.next()?;
                    }
                    Token::Tag {
                        end: false, name, ..
                    } if self.node_name_in(name, &["li"]) => {
                        todo!("li");
                    }
                    Token::Tag {
                        end: false, name, ..
                    } if self.node_name_in(name, &["dd", "dt"]) => {
                        todo!("dt dt");
                    }
                    Token::Tag {
                        end: false, name, ..
                    } if self.node_name_in(name, &["button"]) => {
                        todo!("button");
                    }
                    Token::Tag {
                        end: true, name, ..
                    } if self.node_name_in(
                        name,
                        &[
                            "address",
                            "article",
                            "aside",
                            "blockquote",
                            "button",
                            "center",
                            "details",
                            "dialog",
                            "div",
                            "dl",
                            "fieldset",
                            "figcaption",
                            "figure",
                            "footer",
                            "header",
                            "nav",
                            "ol",
                            "pre",
                            "section",
                            "summary",
                            "ul",
                        ],
                    ) =>
                    {
                        if !self.is_node_in_scope(name) {
                            self.next()?;
                            continue;
                        }
                        self.close_implied_end_tags(&["dd", "dt", "li", "optgroup", "option", "p"]);
                        while let Some(node) = self.open_elements.pop() {
                            let node = self.lexer.dom.get_element_node(node).unwrap();
                            if node.name() == name {
                                break;
                            }
                        }
                        self.next()?;
                    }
                    Token::Tag {
                        end: true, name, ..
                    } if self.node_name_in(name, &["form"]) => {
                        todo!("form end");
                    }
                    Token::Tag {
                        end: true, name, ..
                    } if self.node_name_in(name, &["p"]) => {
                        if !self.is_node_in_button_scope(name) {
                            // otherwise, create a fake p element
                            let mut current = self
                                .lexer
                                .dom
                                .get_element_node_mut(*self.open_elements.last().unwrap())
                                .unwrap();
                            let p = current.append_child_element(name, &[]);
                            self.open_elements.push(p);
                        }
                        self.close_p_node();
                        self.next()?;
                    }
                    Token::Tag {
                        end: false, name, ..
                    } if self.node_name_in(
                        name,
                        &[
                            "caption", "col", "colgroup", "head", "tbody", "td", "tfoot", "th",
                            "thead", "tr",
                        ],
                    ) =>
                    {
                        self.next()?;
                    }
                    Token::Tag {
                        end: false, name, ..
                    } => {
                        // TODO: reconstruct active formatting elements?
                        let mut node = self
                            .lexer
                            .dom
                            .get_element_node_mut(*self.open_elements.last().unwrap())
                            .unwrap();
                        let node = node.append_child_element(name, &self.lexer.attrs);
                        self.open_elements.push(node);
                        self.next()?;
                    }
                    Token::Tag {
                        end: true, name, ..
                    } => {
                        todo!("other end tag");
                    }
                },
                InsertionMode::AfterBody => match self.peek()? {
                    Token::Char(c @ '\t' | c @ '\n' | c @ '\x0C' | c @ ' ') => {
                        // TODO: reconstruct active formatting elements?
                        self.append_text(c);
                        self.next()?;
                    }
                    Token::DocType | Token::Comment => {
                        self.next()?;
                    }
                    Token::Tag {
                        end: false, name, ..
                    } if self.node_name_in(name, &["html"]) => {
                        todo!("html in body")
                    }
                    Token::Tag {
                        end: true, name, ..
                    } if self.node_name_in(name, &["html"]) => {
                        self.insertion_mode = InsertionMode::AfterAfterBody; // yes
                        self.next()?;
                    }
                    Token::Eof => {
                        return self.stop_parsing();
                    }
                    _ => {
                        self.insertion_mode = InsertionMode::InBody;
                    }
                },
                InsertionMode::AfterAfterBody => match self.peek()? {
                    Token::Comment => {
                        self.next()?;
                    }
                    Token::Char(c @ '\t' | c @ '\n' | c @ '\x0C' | c @ ' ') => {
                        // TODO: reconstruct active formatting elements?
                        self.append_text(c);
                        self.next()?;
                    }
                    Token::Tag {
                        end: false, name, ..
                    } if self.node_name_in(name, &["html"]) => {
                        todo!("html after body")
                    }
                    Token::Eof => {
                        return self.stop_parsing();
                    }
                    _ => {
                        self.insertion_mode = InsertionMode::InBody;
                    }
                },
            }
        }
    }
}

enum LexerState {
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

#[derive(Copy, Clone)]
enum Token {
    Eof,
    Char(char),
    Tag {
        self_closing: bool,
        end: bool,
        name: usize,
    },
    DocType,
    Comment,
}

struct Lexer<R> {
    reader: CharReader<R>,
    dom: Dom,
    stash: Option<Result<char, HtmlError>>,
    state: LexerState,
    return_state: LexerState,
    buf: String,
    tok: Token,
    attrs: Vec<[usize; 2]>,
    last_pos: Position,
    pos: Position,
}

impl<R: Read> Lexer<R> {
    fn new(reader: R, dom: Dom) -> Self {
        let mut reader = CharReader::new(reader);
        let stash = reader
            .next()
            .transpose()
            .map_err(HtmlError::from)
            .transpose();
        let pos = Position(0, 0);
        Self {
            reader,
            dom,
            stash,
            state: LexerState::Data,
            return_state: LexerState::Data,
            buf: String::new(),
            tok: Token::Eof,
            attrs: Vec::new(),
            last_pos: pos,
            pos,
        }
    }

    fn skip_whitespace(&mut self) -> Result<(), HtmlError> {
        loop {
            match self.peek_char()? {
                Some(c) if c.is_whitespace() => {
                    self.next()?;
                }
                _ => return Ok(()),
            }
        }
    }

    fn peek_char(&mut self) -> Result<Option<char>, HtmlError> {
        self.stash.clone().transpose()
    }

    fn next_char(&mut self) -> Result<Option<char>, HtmlError> {
        let mut stash = self.stash.take();
        self.last_pos = self.pos;
        self.stash = self
            .reader
            .next()
            .transpose()
            .map_err(HtmlError::from)
            .transpose();

        // normalize newlines
        if let Some(Ok('\r')) = stash {
            if let Some(Ok('\n')) = self.stash {
                // ignore the carriage return if the next char is a newline
                stash = self.stash.take();
                self.stash = self
                    .reader
                    .next()
                    .transpose()
                    .map_err(HtmlError::from)
                    .transpose();
            } else {
                // otherwise, turn the carriage return into a newline
                stash = Some(Ok('\n'));
            }
        }

        match stash {
            None => Ok(None),
            Some(Ok(c)) if c == '\n' => {
                self.pos.0 += 1;
                self.pos.1 = 1;
                Ok(Some(c))
            }
            Some(Ok(c)) => {
                self.pos.1 += 1;
                Ok(Some(c))
            }
            opt => opt.transpose(),
        }
    }

    fn next(&mut self) -> Result<(), HtmlError> {
        loop {
            match self.state {
                LexerState::Data => match self.peek_char()? {
                    Some('&') => todo!("char reference state"),
                    Some('<') => {
                        self.state = LexerState::TagOpen;
                        self.next_char()?;
                    }
                    Some('\x00') => return Err(format!("unexpected null byte").into()),
                    Some(c) => {
                        self.tok = Token::Char(c);
                        self.next_char()?;
                        return Ok(());
                    }
                    None => {
                        self.tok = Token::Eof;
                        return Ok(());
                    }
                },
                LexerState::TagOpen => match self.peek_char()? {
                    Some('!') => todo!("markup decl state"),
                    Some('/') => {
                        self.state = LexerState::EndTagOpen;
                        self.next_char()?;
                    }
                    Some(c) if c.is_ascii_alphabetic() => {
                        self.state = LexerState::TagName;
                        self.tok = Token::Tag {
                            self_closing: false,
                            end: false,
                            name: EMPTY_RANGE_INDEX,
                        };
                        self.buf.clear();
                    }
                    None => return Err(format!("unexpected end of file").into()),
                    Some(c) => return Err(format!("unexpected `{c}` instead of tag name").into()),
                },
                LexerState::EndTagOpen => match self.peek_char()? {
                    Some(c) if c.is_ascii_alphabetic() => {
                        self.state = LexerState::TagName;
                        self.tok = Token::Tag {
                            self_closing: false,
                            end: true,
                            name: EMPTY_RANGE_INDEX,
                        };
                        self.buf.clear();
                    }
                    Some('>') => return Err(format!("missing end tag name").into()),
                    None => return Err(format!("unexpected end of file").into()),
                    Some(c) => return Err(format!("unexpected `{c}` instead of tag name").into()),
                },
                LexerState::TagName => match self.peek_char()? {
                    Some('\t' | '\n' | '\x0C' | ' ') => {
                        // need to set tag name if we haven't yet
                        if let Token::Tag {
                            self_closing,
                            end,
                            name,
                        } = self.tok
                        {
                            if name == EMPTY_RANGE_INDEX {
                                let name = self.dom.insert_text(&self.buf);
                                self.tok = Token::Tag {
                                    self_closing,
                                    end,
                                    name,
                                };
                            }
                        }
                        self.state = LexerState::BeforeAttributeName;
                        self.next_char()?;
                    }
                    Some('/') => {
                        self.state = LexerState::SelfClosingStartTag;
                        self.next_char()?;
                    }
                    Some('>') => {
                        // need to set tag name if we haven't yet
                        if let Token::Tag {
                            self_closing,
                            end,
                            name,
                        } = self.tok
                        {
                            if name == EMPTY_RANGE_INDEX {
                                let name = self.dom.insert_text(&self.buf);
                                self.tok = Token::Tag {
                                    self_closing,
                                    end,
                                    name,
                                };
                            }
                        }
                        self.state = LexerState::Data;
                        self.next_char()?;
                        return Ok(());
                    }
                    Some(c) if c.is_ascii_uppercase() => {
                        self.buf.push(c.to_ascii_lowercase());
                        self.next_char()?;
                    }
                    Some('\x00') => return Err(format!("unexpected null byte").into()),
                    None => return Err(format!("unexpected end of file").into()),
                    Some(c) => {
                        self.buf.push(c);
                        self.next_char()?;
                    }
                },
                LexerState::BeforeAttributeName => match self.peek_char()? {
                    Some('\t' | '\n' | '\x0C' | ' ') => {
                        self.next_char()?;
                    }
                    None | Some('/' | '>') => self.state = LexerState::AfterAttributeName,
                    Some('=') => return Err(format!("unexpected `=` before attribute name").into()),
                    Some(_) => {
                        self.state = LexerState::AttributeName;
                        self.buf.clear();
                        self.attrs.clear();
                        self.attrs.push([EMPTY_RANGE_INDEX, EMPTY_RANGE_INDEX]);
                    }
                },
                LexerState::AttributeName => match self.peek_char()? {
                    None | Some('\t' | '\n' | '\x0C' | ' ' | '/' | '>') => {
                        self.state = LexerState::AfterAttributeName;
                        let name = self.dom.insert_text(&self.buf);
                        self.attrs.last_mut().unwrap()[0] = name;
                    }
                    Some('=') => {
                        self.state = LexerState::BeforeAttributeValue;
                        let name = self.dom.insert_text(&self.buf);
                        self.attrs.last_mut().unwrap()[0] = name;
                        self.next_char()?;
                    }
                    Some(c) if c.is_ascii_uppercase() => {
                        self.buf.push(c.to_ascii_lowercase());
                        self.next_char()?;
                    }
                    Some('\x00') => return Err(format!("unexpected null byte").into()),
                    Some(c @ '"' | c @ '\'' | c @ '<') => {
                        return Err(format!("unexpected `{c}` in attribute name").into())
                    }
                    Some(c) => {
                        self.buf.push(c);
                        self.next_char()?;
                    }
                },
                LexerState::AfterAttributeName => match self.peek_char()? {
                    Some('\t' | '\n' | '\x0C' | ' ') => {
                        self.next_char()?;
                    }
                    Some('/') => {
                        self.state = LexerState::SelfClosingStartTag;
                        self.next_char()?;
                    }
                    Some('=') => {
                        self.state = LexerState::BeforeAttributeValue;
                        self.next_char()?;
                    }
                    Some('>') => {
                        self.state = LexerState::Data;
                        self.next_char()?;
                    }
                    None => return Err(format!("unexpected end of file").into()),
                    Some(_) => {
                        self.buf.clear();
                        self.attrs.push([EMPTY_RANGE_INDEX, EMPTY_RANGE_INDEX]);
                    }
                },
                LexerState::BeforeAttributeValue => match self.peek_char()? {
                    Some('\t' | '\n' | '\x0C' | ' ') => {
                        self.next_char()?;
                    }
                    Some('"') => {
                        self.state = LexerState::AttributeValueDoubleQuote;
                        self.buf.clear();
                        self.next_char()?;
                    }
                    Some('\'') => {
                        self.state = LexerState::AttributeValueSingleQuote;
                        self.buf.clear();
                        self.next_char()?;
                    }
                    Some('>') => return Err(format!("missing attribute value").into()),
                    _ => {
                        self.state = LexerState::AttributeValueNoQuote;
                        self.buf.clear();
                    }
                },
                LexerState::AttributeValueDoubleQuote => match self.peek_char()? {
                    Some('"') => {
                        self.state = LexerState::AfterAttributeValueQuoted;
                        let value = self.dom.insert_text(&self.buf);
                        self.attrs.last_mut().unwrap()[1] = value;
                        self.next_char()?;
                    }
                    Some('&') => {
                        self.return_state = LexerState::AttributeValueDoubleQuote;
                        todo!("char reference state");
                        self.next_char()?;
                    }
                    Some('\x00') => return Err(format!("unexpected null byte").into()),
                    None => return Err(format!("unexpected end of file").into()),
                    Some(c) => {
                        self.buf.push(c);
                        self.next_char()?;
                    }
                },
                LexerState::AttributeValueSingleQuote => match self.peek_char()? {
                    Some('\'') => {
                        self.state = LexerState::AfterAttributeValueQuoted;
                        let value = self.dom.insert_text(&self.buf);
                        self.attrs.last_mut().unwrap()[1] = value;
                        self.next_char()?;
                    }
                    Some('&') => {
                        self.return_state = LexerState::AttributeValueSingleQuote;
                        todo!("char reference state");
                        self.next_char()?;
                    }
                    Some('\x00') => return Err(format!("unexpected null byte").into()),
                    None => return Err(format!("unexpected end of file").into()),
                    Some(c) => {
                        self.buf.push(c);
                        self.next_char()?;
                    }
                },
                LexerState::AttributeValueNoQuote => match self.peek_char()? {
                    Some('\t' | '\n' | '\x0C' | ' ') => {
                        self.state = LexerState::BeforeAttributeName;
                        let value = self.dom.insert_text(&self.buf);
                        self.attrs.last_mut().unwrap()[1] = value;
                        self.next_char()?;
                    }
                    Some('&') => {
                        self.return_state = LexerState::AttributeValueNoQuote;
                        todo!("char reference state");
                        self.next_char()?;
                    }
                    Some('>') => {
                        self.state = LexerState::Data;
                        let value = self.dom.insert_text(&self.buf);
                        self.attrs.last_mut().unwrap()[1] = value;
                        self.next_char()?;
                        return Ok(());
                    }
                    Some('\x00') => return Err(format!("unexpected null byte").into()),
                    Some(c @ '"' | c @ '\'' | c @ '<' | c @ '=' | c @ '`') => {
                        return Err(format!("unexpected `{c}` in attribute value").into())
                    }
                    None => return Err(format!("unexpected end of file").into()),
                    Some(c) => {
                        self.buf.push(c);
                        self.next_char()?;
                    }
                },
                LexerState::AfterAttributeValueQuoted => match self.peek_char()? {
                    Some('\t' | '\n' | '\x0C' | ' ') => {
                        self.state = LexerState::BeforeAttributeName;
                        self.next_char()?;
                    }
                    Some('/') => {
                        self.state = LexerState::SelfClosingStartTag;
                        self.next_char()?;
                    }
                    Some('>') => {
                        self.state = LexerState::Data;
                        return Ok(());
                    }
                    None => return Err(format!("unexpected end of file").into()),
                    Some(_) => return Err(format!("missing white space between attributes").into()),
                },
                LexerState::SelfClosingStartTag => match self.peek_char()? {
                    Some('>') => {
                        if let Token::Tag { end, mut name, .. } = self.tok {
                            // need to set tag name if we haven't yet
                            if name == EMPTY_RANGE_INDEX {
                                name = self.dom.insert_text(&self.buf);
                            }
                            self.tok = Token::Tag {
                                self_closing: true,
                                end,
                                name,
                            };
                        }
                        self.state = LexerState::Data;
                        self.next_char()?;
                        return Ok(());
                    }
                    None => return Err(format!("unexpected end of file").into()),
                    Some(_) => return Err(format!("unexpected `/` in tag").into()),
                },
            }
        }
    }
}

#[derive(thiserror::Error, Debug)]
enum CharReaderError {
    #[error("{0}")]
    IoError(#[from] io::Error),

    #[error("{0}")]
    Utf8Error(#[from] str::Utf8Error),
}

struct CharReader<R> {
    inner: R,
    buf: [u8; 4],
    buf_len: usize,
}

impl<R> CharReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            buf: [0; 4],
            buf_len: 0,
        }
    }
}

impl<R: Read> Iterator for CharReader<R> {
    type Item = Result<char, CharReaderError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.buf_len == 0 {
            self.buf_len = match self.inner.read(&mut self.buf) {
                Ok(len) => len,
                Err(e) => return Some(Err(e.into())),
            }
        }

        if self.buf_len == 0 {
            return None;
        }

        let s = match str::from_utf8(&self.buf[0..self.buf_len]) {
            Ok(s) => s,
            Err(e) => {
                let valid_len = e.valid_up_to();
                if valid_len == 0 {
                    return Some(Err(e.into()));
                }
                // Safety: We already checked up to `valid_len`
                unsafe { str::from_utf8_unchecked(&self.buf[0..valid_len]) }
            }
        };

        let c = s.chars().next().unwrap();
        let char_len = c.len_utf8();
        self.buf.rotate_left(char_len);
        self.buf_len -= char_len;
        Some(Ok(c))
    }
}
