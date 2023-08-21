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
