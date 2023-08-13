use std::{
    collections::VecDeque,
    io::{self, Read},
    ops::Range,
};

const SEQUENCE_THRESHOLD: usize = 8;
const LONGEST_SEQUENCE: usize = 40; // (2^5) + SEQUENCE_THRESHOLD
const WINDOW_SIZE: usize = 2048; // (2^11)

enum Token {
    Byte(u8),
    Ref(u16), // 11 bit offset, 5 bit len
}

struct LzssDecompressor<R> {
    src: R,
    buf: VecDeque<u8>,
}

pub struct LzssCompressor<R> {
    src: R,
    window: VecDeque<u8>,
    tokens: VecDeque<Token>,
    in_buf: VecDeque<u8>,
    out_buf: VecDeque<u8>,
}

impl<R: Read> LzssCompressor<R> {
    pub fn new(src: R) -> Self {
        Self {
            src,
            window: VecDeque::new(),
            tokens: VecDeque::new(),
            in_buf: VecDeque::new(),
            out_buf: VecDeque::new(),
        }
    }

    fn fill_tokens(&mut self) -> io::Result<()> {
        while self.tokens.len() < 8 {
            // first get some bytes into the in_buf
            if self.in_buf.len() < LONGEST_SEQUENCE {
                let mut buf = [0; LONGEST_SEQUENCE];
                let len = self.src.read(&mut buf)?;
                self.in_buf.extend(&buf[0..len]);
            }

            // if in_buf is empty, we can't read any more tokens ever
            if self.in_buf.is_empty() {
                return Ok(());
            }

            /// find range of longest prefix of needle in haystack
            fn longest_prefix_match(needle: &[u8], haystack: &[u8]) -> Option<Range<usize>> {
                if needle.len() < SEQUENCE_THRESHOLD {
                    return None;
                }

                let mut longest_start = 0;
                let mut longest_len = 0;

                for (start, window) in haystack
                    .windows(needle.len().min(LONGEST_SEQUENCE))
                    .enumerate()
                {
                    for i in 0..window.len() {
                        if needle[i] != window[i] {
                            break;
                        }
                        if (i + 1) > longest_len {
                            longest_start = start;
                            longest_len = i + 1;
                        }
                    }
                }

                if longest_len > 0 {
                    return Some(longest_start..(longest_start + longest_len));
                }

                None
            }
            let match_range =
                longest_prefix_match(self.in_buf.make_contiguous(), self.window.make_contiguous());

            // add token
            let len = if let Some(range) = match_range && range.len() > SEQUENCE_THRESHOLD {
                let word = ((range.start << 5) | (range.len() - SEQUENCE_THRESHOLD)) as u16;
                self.tokens.push_back(Token::Ref(word));
                range.len()
            } else {
                self.tokens
                    .push_back(Token::Byte(*self.in_buf.front().unwrap()));
                1
            };

            // shift matched in_buf into the window
            for _ in 0..len {
                self.window.push_back(self.in_buf.pop_front().unwrap());
            }
            while self.window.len() > WINDOW_SIZE {
                self.window.pop_front();
            }
        }
        Ok(())
    }

    fn fill_out_buf(&mut self) {
        // flag byte:
        // for a string of <= 8 tokens, set a bit to 1 if the token if a byte
        assert!(self.tokens.len() <= 8);
        if self.tokens.is_empty() {
            return;
        }

        let mut shift = 0;
        let mut flags = 0;
        for token in self.tokens.drain(..) {
            match token {
                Token::Byte(b) => {
                    flags |= 1 << shift;
                    self.out_buf.push_back(b);
                }
                Token::Ref(w) => {
                    for b in w.to_le_bytes() {
                        self.out_buf.push_back(b);
                    }
                }
            }
            shift += 1;
        }
        self.out_buf.push_front(flags);
    }
}

impl<R: Read> Read for LzssCompressor<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.out_buf.is_empty() {
            self.fill_tokens()?;
            self.fill_out_buf();
        }
        if self.out_buf.is_empty() {
            return Ok(0);
        }

        let len = buf.len().min(self.out_buf.len());
        for i in 0..len {
            buf[i] = self.out_buf.pop_front().unwrap();
        }
        Ok(len)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn compress() {
        let input = Cursor::new("This is a test! Hello. This is a test?");
        let mut reader = LzssCompressor::new(input);
        let mut output = Vec::new();
        reader.read_to_end(&mut output).unwrap();
        assert_eq!(
            b"\xFFThis is \xFFa test! \x7FHello. \x06\x00\x01?",
            output.as_slice()
        );
    }
}
