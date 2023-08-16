use std::iter::Peekable;

/// An iterator of `(usize, char)` that converts consective `\r\n` characters
/// and all lone `\r` characters into `\n`. The `usize` part of
/// each item indicates how many characters where normalized in the process
/// of returning this character.
pub struct NewlineNormalized<I: Iterator> {
    inner: Peekable<I>,
}

pub trait NewlineNormalizedConsume {
    fn consume(&mut self) -> usize;
}

pub trait NewlineNormalizable {
    fn newline_normalized(self) -> NewlineNormalized<Self>
    where
        Self: Sized + Iterator;
}

impl<I: Iterator<Item = char>> NewlineNormalizable for I {
    /// Turns an iterator of `char` into a `NewlineNormalized`.
    fn newline_normalized(self) -> NewlineNormalized<Self> {
        NewlineNormalized {
            inner: self.peekable(),
        }
    }
}

impl<I: Iterator<Item = char>> Iterator for NewlineNormalized<I> {
    type Item = (usize, char);
    fn next(&mut self) -> Option<Self::Item> {
        let c = self.inner.next();
        if let Some('\r') = c {
            // ignore <CR> if next char is <LF>
            if let Some('\n') = self.inner.peek() {
                self.inner.next();
                return Some((2, '\n'));
            }
            // turn <CR> into <LF>
            return Some((1, '\n'));
        }
        c.map(|c| (1, c))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanity_test() {
        let mut iter = "te\rst\r\n".chars().newline_normalized();
        assert_eq!(Some((1, 't')), iter.next());
        assert_eq!(Some((1, 'e')), iter.next());
        assert_eq!(Some((1, '\n')), iter.next());
        assert_eq!(Some((1, 's')), iter.next());
        assert_eq!(Some((1, 't')), iter.next());
        assert_eq!(Some((2, '\n')), iter.next());
        assert_eq!(None, iter.next());
    }
}
