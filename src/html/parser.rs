use super::tokenizer::Tokenizer;
use crate::dom::Dom;

#[derive(thiserror::Error, Debug, Clone)]
#[error("{msg}")]
pub struct ParserError {
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

#[pin_project::pin_project]
pub struct Parser<R> {
    #[pin]
    tokenizer: Tokenizer<R, Dom>,
}

impl<R> Parser<R> {
    pub async fn parse(&mut self) -> Result<(), ParserError> {
        todo!()
    }
}
