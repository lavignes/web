use super::tokenizer::Tokenizer;
use crate::dom::Dom;

#[pin_project::pin_project]
pub struct Parser<R> {
    #[pin]
    tokenizer: Tokenizer<R, Dom>,
}
