use failure::Fail;

pub struct AsciiFile<'m> {
    mapping: &'m [u8],
}

pub type AsciiFileIterator<'t> = PositionedChars<std::str::Chars<'t>>;

#[derive(Debug, Fail)]
pub enum EncodingError {
    #[fail(
        display = "input contains non-ascii character at byte offset {}: ...{}<?>",
        position,
        prev
    )]
    NotAscii { position: usize, prev: String },
}

const ENCODING_ERROR_MAX_CONTEXT_LEN: usize = 180;

impl<'m> AsciiFile<'m> {
    /// return the start index of the line without the newline character (\n).
    /// This means you can make slices that do NOT include the newline
    /// character by simply using the return value of this function as
    /// lower bound X in the range X..Y.
    fn get_line_end_idx(mapping: &'m [u8], byte_offset: usize) -> usize {
        let region_end = mapping
            .len()
            .min(byte_offset + ENCODING_ERROR_MAX_CONTEXT_LEN);

        let region = &mapping[byte_offset..region_end];
        region
            .iter()
            .position(|&chr| chr == '\n' as u8)
            .map(|pos| pos + byte_offset)
            .unwrap_or(region_end)
    }

    /// return the end index of the line includes the newline character (\n).
    /// This means you can make slices that do NOT include the newline
    /// character by simply using the return value of this function as
    /// upper bound Y in the range X..Y.
    fn get_line_start_idx(mapping: &'m [u8], byte_offset: usize) -> usize {
        let region_start = byte_offset
            .checked_sub(ENCODING_ERROR_MAX_CONTEXT_LEN)
            .unwrap_or(0);

        let region = &mapping[region_start..byte_offset];

        region
            .iter()
            .position(|&chr| chr == '\n' as u8)
            .map(|pos| pos + 1)
            .unwrap_or(region_start)
    }

    // cost: O(fileLen) since we need to check if all chars are ASCII
    pub fn new(mapping: &'m [u8]) -> Result<AsciiFile<'m>, EncodingError> {
        if let Some(position) = mapping.iter().position(|c| !c.is_ascii()) {
            // TODO: add max context length :)
            let start_idx = AsciiFile::get_line_start_idx(mapping, position);
            debug_assert!(position >= start_idx);
            // We know everything until now has been ASCII
            let prev: &str =
                unsafe { std::str::from_utf8_unchecked(&mapping[start_idx..position]) };
            let prev = prev.to_owned();
            return Err(EncodingError::NotAscii { position, prev });
        }

        Ok(AsciiFile { mapping })
    }

    pub fn iter(&self) -> AsciiFileIterator<'_> {
        let s: &str = self;
        PositionedChars {
            curpos: Position {
                row: 0,
                col: 0,
                byte_offset: 0,
            },
            ascii_file: s.chars(),
            peeked: String::new(),
        }
    }
}

use std::ops::Deref;

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Position {
    pub row: usize,
    pub col: usize,
    byte_offset: usize,
}

use std::fmt::{self, Display};

impl Display for Position {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(fmt, "{}:{}", self.row + 1, self.col + 1)
    }
}

#[derive(Debug, PartialEq)]
pub struct PositionedChar(pub Position, pub char);

pub struct PositionedChars<I>
where
    I: Iterator<Item = char>,
{
    curpos: Position,
    ascii_file: I,
    peeked: String,
}

impl<I> Iterator for PositionedChars<I>
where
    I: Iterator<Item = char>,
{
    type Item = PositionedChar;
    fn next(&mut self) -> Option<PositionedChar> {
        let c = self.peek()?;
        self.peeked.remove(0);

        let retpos = self.curpos;
        self.curpos.byte_offset += 1;
        self.curpos.col += 1;
        if c == '\n' {
            self.curpos.row += 1;
            self.curpos.col = 0;
        }

        Some(PositionedChar(retpos, c))
    }
}

impl<I> PositionedChars<I>
where
    I: Iterator<Item = char>,
{
    pub fn try_peek_multiple(&mut self, n: usize) -> Option<&str> {
        let peeked = self.peek_multiple(n);
        if peeked.len() < n {
            None
        } else {
            Some(peeked)
        }
    }

    /// Peek the next n characters, without advancing the iteratior (it is
    /// actually advanced, of course, but this is hidden by a buffer).
    /// If there are less than n charcters left, the returned slice is shorter
    /// than that
    pub fn peek_multiple(&mut self, n: usize) -> &str {
        // We already have .len() characters "in stock", so we get the remaining n-len,
        // if there are then many
        for _ in self.peeked.len()..n {
            match self.ascii_file.next() {
                Some(next) => self.peeked.push(next),
                None => break,
            }
        }

        &self.peeked[0..self.peeked.len().min(n)]
    }

    /// Can't use peekable, because we don't care about position when peeking
    pub fn peek(&mut self) -> Option<char> {
        self.peek_multiple(1).chars().next()
    }

    pub fn eof_reached(&mut self) -> bool {
        self.peek().is_none()
    }

    pub fn current_position(&mut self) -> Position {
        self.curpos
    }
}

impl<'m> Deref for AsciiFile<'m> {
    type Target = str;
    fn deref(&self) -> &Self::Target {
        unsafe { std::str::from_utf8_unchecked(&self.mapping) }
    }
}

impl<'m, 'a> Into<&'m str> for &'a AsciiFile<'m> {
    fn into(self) -> &'m str {
        unsafe { std::str::from_utf8_unchecked(&self.mapping) }
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    fn testfile(s: &str) -> &[u8] {
        s.as_bytes()
    }

    #[test]
    fn works_with_ascii() {
        let instr = "ABCDEFG\n\t";
        let f = testfile(instr);
        let af = AsciiFile::new(f).unwrap();
        let s: &str = &af;
        assert!(instr == s);
    }

    #[test]
    fn returns_err_on_non_ascii() {
        let f = testfile("one💩two");
        let mm = AsciiFile::new(f);
        assert!(mm.is_err());
        let e = mm.err().unwrap();
        println!("{:?}", e);
        let EncodingError::NotAscii { position, prev } = e;
        assert_eq!(position, 3);
        assert_eq!(prev, "one");
    }

    #[test]
    fn returns_err_on_non_ascii_non_utf8() {
        let input: Vec<u16> = "ä".encode_utf16().collect();
        let input: &[u8] =
            unsafe { std::slice::from_raw_parts(input.as_ptr() as *const u8, 2 * input.len()) };
        assert_eq!(input.len(), 2);
        match std::str::from_utf8(input) {
            Ok(s) => panic!("test implementation error {:?}", s),
            Err(x) => println!("ä in utf16 is not valid utf8, but let's be sure {:?}", x),
        }
        let r = AsciiFile::new(&input);
        assert!(r.is_err());
        let EncodingError::NotAscii { position, prev } = r.err().unwrap();
        assert_eq!(position, 0);
        assert_eq!(prev, "");
    }

    #[test]
    fn err_on_ascii_context_only_current_line() {
        let f = testfile("0\n12345💩");
        let mm = AsciiFile::new(f);
        assert!(mm.is_err());
        let e = mm.err().unwrap();
        println!("{:?}", e);
        let EncodingError::NotAscii { prev, .. } = e;
        assert_eq!(prev, "12345");
    }

    #[test]
    fn err_on_ascii_context_bounded() {
        use std::fmt::Write;

        let mut prev_expected = "9876543210".repeat(ENCODING_ERROR_MAX_CONTEXT_LEN / 10);
        let instr = format!("abcd{}💩", prev_expected);
        let f = testfile(&instr);
        let mm = AsciiFile::new(f);

        assert!(mm.is_err());

        let e = mm.err().unwrap();
        let EncodingError::NotAscii { prev, .. } = e;

        assert_eq!(prev, prev_expected);
    }

    #[test]
    fn test_indices() {
        let mut single_line = "|123456789";

        assert_eq!(
            single_line,
            &single_line[..AsciiFile::get_line_end_idx(single_line.as_bytes(), 5)]
        );
        assert_eq!(
            single_line,
            &single_line[AsciiFile::get_line_start_idx(single_line.as_bytes(), 5)..]
        );

        let mut multi_line = "|123456789\nabcdefghijklmno";

        assert_eq!(
            multi_line,
            &multi_line[..AsciiFile::get_line_end_idx(multi_line.as_bytes(), 15)]
        );
        assert_eq!(
            "|123456789",
            &multi_line[..AsciiFile::get_line_end_idx(multi_line.as_bytes(), 5)]
        );
        assert_eq!(
            "abcdefghijklmno",
            &multi_line[AsciiFile::get_line_start_idx(multi_line.as_bytes(), 15)..]
        );

        assert_eq!(
            multi_line,
            &multi_line[AsciiFile::get_line_start_idx(multi_line.as_bytes(), 5)..]
        );

        let mut long_line = "|123456789\n".repeat(ENCODING_ERROR_MAX_CONTEXT_LEN);

        assert_eq!(
            "|123456789",
            &long_line[AsciiFile::get_line_start_idx(long_line.as_bytes(), 20)
                ..AsciiFile::get_line_end_idx(long_line.as_bytes(), 20)]
        );

        let mut empty_line = "\n\n\n".repeat(ENCODING_ERROR_MAX_CONTEXT_LEN);

        assert_eq!(
            "",
            &empty_line[AsciiFile::get_line_start_idx(empty_line.as_bytes(), 1)
                ..AsciiFile::get_line_end_idx(empty_line.as_bytes(), 1)]
        );
    }

    #[test]
    fn iterator_works() {
        let f = testfile("one\ntwo three\nfour\n\n");
        let af = AsciiFile::new(f).unwrap();
        let res: Vec<PositionedChar> = af.iter().collect();

        #[rustfmt::skip]
        let exp = vec![
            PositionedChar(Position { byte_offset:  0, row: 0, col: 0 }, 'o'),
            PositionedChar(Position { byte_offset:  1, row: 0, col: 1 }, 'n'),
            PositionedChar(Position { byte_offset:  2, row: 0, col: 2 }, 'e'),
            PositionedChar(Position { byte_offset:  3, row: 0, col: 3 }, '\n'),
            PositionedChar(Position { byte_offset:  4, row: 1, col: 0 }, 't'),
            PositionedChar(Position { byte_offset:  5, row: 1, col: 1 }, 'w'),
            PositionedChar(Position { byte_offset:  6, row: 1, col: 2 }, 'o'),
            PositionedChar(Position { byte_offset:  7, row: 1, col: 3 }, ' '),
            PositionedChar(Position { byte_offset:  8, row: 1, col: 4 }, 't'),
            PositionedChar(Position { byte_offset:  9, row: 1, col: 5 }, 'h'),
            PositionedChar(Position { byte_offset: 10, row: 1, col: 6 }, 'r'),
            PositionedChar(Position { byte_offset: 11, row: 1, col: 7 }, 'e'),
            PositionedChar(Position { byte_offset: 12, row: 1, col: 8 }, 'e'),
            PositionedChar(Position { byte_offset: 13, row: 1, col: 9 }, '\n'),
            PositionedChar(Position { byte_offset: 14, row: 2, col: 0 }, 'f'),
            PositionedChar(Position { byte_offset: 15, row: 2, col: 1 }, 'o'),
            PositionedChar(Position { byte_offset: 16, row: 2, col: 2 }, 'u'),
            PositionedChar(Position { byte_offset: 17, row: 2, col: 3 }, 'r'),
            PositionedChar(Position { byte_offset: 18, row: 2, col: 4 }, '\n'),
            PositionedChar(Position { byte_offset: 19, row: 3, col: 0 }, '\n'),
        ];
        assert_eq!(exp, res);
    }

    #[test]
    fn peeking_works() {
        let f = testfile("one\ntwo three\nfour\n");
        let af = AsciiFile::new(f).unwrap();
        let mut i = af.iter();

        let mut peeked = i.peek();
        while let Some(PositionedChar(_, c)) = i.next() {
            assert_eq!(c, peeked.unwrap());
            peeked = i.peek();
        }
    }

    #[test]
    fn multi_peeking_works() {
        let s = "one\ntwo three\nfour\n\n";
        let f = testfile(s);
        let af = AsciiFile::new(f).unwrap();
        let mut i = af.iter();

        let peeked = i.peek_multiple(s.len());

        assert_eq!(s, peeked);

        #[rustfmt::skip]
        let exp = vec![
            PositionedChar(Position { byte_offset:  0, row: 0, col: 0 }, 'o'),
            PositionedChar(Position { byte_offset:  1, row: 0, col: 1 }, 'n'),
            PositionedChar(Position { byte_offset:  2, row: 0, col: 2 }, 'e'),
            PositionedChar(Position { byte_offset:  3, row: 0, col: 3 }, '\n'),
            PositionedChar(Position { byte_offset:  4, row: 1, col: 0 }, 't'),
            PositionedChar(Position { byte_offset:  5, row: 1, col: 1 }, 'w'),
            PositionedChar(Position { byte_offset:  6, row: 1, col: 2 }, 'o'),
            PositionedChar(Position { byte_offset:  7, row: 1, col: 3 }, ' '),
            PositionedChar(Position { byte_offset:  8, row: 1, col: 4 }, 't'),
            PositionedChar(Position { byte_offset:  9, row: 1, col: 5 }, 'h'),
            PositionedChar(Position { byte_offset: 10, row: 1, col: 6 }, 'r'),
            PositionedChar(Position { byte_offset: 11, row: 1, col: 7 }, 'e'),
            PositionedChar(Position { byte_offset: 12, row: 1, col: 8 }, 'e'),
            PositionedChar(Position { byte_offset: 13, row: 1, col: 9 }, '\n'),
            PositionedChar(Position { byte_offset: 14, row: 2, col: 0 }, 'f'),
            PositionedChar(Position { byte_offset: 15, row: 2, col: 1 }, 'o'),
            PositionedChar(Position { byte_offset: 16, row: 2, col: 2 }, 'u'),
            PositionedChar(Position { byte_offset: 17, row: 2, col: 3 }, 'r'),
            PositionedChar(Position { byte_offset: 18, row: 2, col: 4 }, '\n'),
            PositionedChar(Position { byte_offset: 19, row: 3, col: 0 }, '\n'),
        ];

        let res: Vec<PositionedChar> = i.collect();

        assert_eq!(exp, res);
    }

}
