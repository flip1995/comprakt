//! The diagnostics object controls the output of warnings and errors generated
//! by the compiler during the lexing, parsing and semantic analysis phases.
//! It also tracks the number of warnings and errors generated for flow control.
//!
//! This implementation is NOT thread-safe.
use asciifile::{MaybeSpanned, Span, Spanned};
use crate::color::ColorOutput;
use failure::Error;
use std::{ascii::escape_default, cell::RefCell, collections::HashMap, fmt::Display};
use termcolor::{Color, WriteColor};

pub fn u8_to_printable_representation(byte: u8) -> String {
    let bytes = escape_default(byte).collect::<Vec<u8>>();
    let rep = unsafe { std::str::from_utf8_unchecked(&bytes) };
    rep.to_owned()
}

/// This abstraction allows us to call the diagnostics API with pretty
/// much everything.
pub trait Printable<'a, 'b> {
    fn as_maybe_spanned(&'b self) -> MaybeSpanned<'a, &'b dyn Display>;
}

// TODO: implementing on `str` (which is what you would like to do, to
// support calls with warning("aa") instead of warning(&"aa").
impl<'a, 'b> Printable<'a, 'b> for &'b str {
    fn as_maybe_spanned(&'b self) -> MaybeSpanned<'a, &'b dyn Display> {
        MaybeSpanned::WithoutSpan(self)
    }
}

impl<'a, 'b, T: Display + 'b> Printable<'a, 'b> for Spanned<'a, T> {
    fn as_maybe_spanned(&'b self) -> MaybeSpanned<'a, &'b dyn Display> {
        MaybeSpanned::WithSpan(Spanned {
            span: self.span.clone(),
            data: &self.data,
        })
    }
}

impl<'a, 'b, T: Display + 'b> Printable<'a, 'b> for MaybeSpanned<'a, T> {
    fn as_maybe_spanned(&'b self) -> MaybeSpanned<'a, &'b dyn Display> {
        match self {
            MaybeSpanned::WithSpan(ref spanned) => MaybeSpanned::WithSpan(Spanned {
                span: spanned.span.clone(),
                data: &spanned.data,
            }),
            MaybeSpanned::WithoutSpan(ref data) => MaybeSpanned::WithoutSpan(data),
        }
    }
}

/// Width of tabs in error and warning messages
const TAB_WIDTH: usize = 4;

/// Color used for rendering line numbers, escape sequences
/// and others...
const HIGHLIGHT_COLOR: Option<Color> = Some(Color::Cyan);

// TODO reimplement line truncation

/// Instead of writing errors, warnings and lints generated in the different
/// compiler stages directly to stdout, they are collected in this object.
///
/// This has several advantages:
/// - the output level can be adapted by users.
/// - we have a single source responsible for formatting compiler messages.
pub struct Diagnostics {
    message_count: RefCell<HashMap<MessageLevel, usize>>,
    writer: RefCell<Box<dyn WriteColor>>,
}

impl Diagnostics {
    pub fn new(writer: Box<dyn WriteColor>) -> Self {
        Self {
            writer: RefCell::new(writer),
            message_count: RefCell::new(HashMap::new()),
        }
    }

    /// True when an error message was emitted, false
    /// if only warnings were emitted.
    pub fn errored(&self) -> bool {
        self.message_count
            .borrow()
            .get(&MessageLevel::Error)
            .is_some()
    }

    pub fn count(&self, level: MessageLevel) -> usize {
        self.message_count
            .borrow()
            .get(&level)
            .cloned()
            .unwrap_or(0)
    }

    pub fn write_statistics(&self) {
        let mut writer = self.writer.borrow_mut();
        let mut output = ColorOutput::new(&mut **writer);

        output.set_bold(true);

        if self.errored() {
            output.set_color(MessageLevel::Error.color());
            writeln!(
                output.writer(),
                "Compilation aborted due to {}",
                match self.count(MessageLevel::Error) {
                    1 => "an error".to_string(),
                    n => format!("{} errors", n),
                }
            );
        } else {
            output.set_color(Some(Color::Green));
            writeln!(
                output.writer(),
                "Compilation finished successfully {}",
                match self.count(MessageLevel::Warning) {
                    0 => "without warnings".to_string(),
                    1 => "with a warning".to_string(),
                    n => format!("with {} warnings", n),
                }
            );
        }
    }

    /// Generate an error or a warning that is printed to the
    /// writer given in the `new` constructor. Most of the time
    /// this will be stderr.
    pub fn emit(&self, level: MessageLevel, kind: MaybeSpanned<'_, &dyn Display>) {
        self.increment_level_count(level);
        let mut writer = self.writer.borrow_mut();
        let msg = Message { level, kind };

        msg.write(&mut **writer);
    }

    #[allow(dead_code)]
    pub fn warning<'a, 'b, T: Printable<'a, 'b> + ?Sized>(&self, kind: &'b T) {
        self.emit(MessageLevel::Warning, kind.as_maybe_spanned())
    }

    #[allow(dead_code)]
    pub fn error<'a, 'b, T: Printable<'a, 'b> + ?Sized>(&self, kind: &'b T) {
        self.emit(MessageLevel::Error, kind.as_maybe_spanned())
    }

    #[allow(dead_code)]
    pub fn info<'a, 'b, T: Printable<'a, 'b> + ?Sized>(&self, kind: &'b T) {
        self.emit(MessageLevel::Info, kind.as_maybe_spanned())
    }

    fn increment_level_count(&self, level: MessageLevel) {
        let mut message_count = self.message_count.borrow_mut();
        let counter = message_count.entry(level).or_insert(0);
        *counter += 1;
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Hash)]
pub enum MessageLevel {
    Error,
    Warning,
    Info,
}

impl MessageLevel {
    fn color(self) -> Option<Color> {
        // Don't be confused by the return type.
        // `None` means default color in the colorterm
        // crate!
        match self {
            MessageLevel::Error => Some(Color::Red),
            MessageLevel::Warning => Some(Color::Yellow),
            MessageLevel::Info => Some(Color::Cyan),
        }
    }

    fn name(&self) -> &str {
        match self {
            MessageLevel::Error => "error",
            MessageLevel::Warning => "warning",
            MessageLevel::Info => "info",
        }
    }
}

pub struct Message<'file, 'msg> {
    pub level: MessageLevel,
    pub kind: MaybeSpanned<'file, &'msg dyn Display>,
}

impl<'file, 'msg> Message<'file, 'msg> {
    pub fn write(&self, writer: &mut dyn WriteColor) {
        match &self.kind {
            MaybeSpanned::WithoutSpan(_) => {
                // TODO: we are surpressing the io error here
                self.write_description(writer).ok();
            }

            MaybeSpanned::WithSpan(spanned) => {
                // TODO: we are surpressing the io error here
                self.write_description(writer).ok();
                self.write_code(writer, &spanned.span).ok();
            }
        }

        // TODO: we are surpressing the io error here
        writeln!(writer).ok();
    }

    fn write_description(&self, writer: &mut dyn WriteColor) -> Result<(), Error> {
        let mut output = ColorOutput::new(writer);
        output.set_color(self.level.color());
        output.set_bold(true);
        write!(output.writer(), "{}: ", self.level.name())?;

        output.set_color(None);
        writeln!(output.writer(), "{}", *self.kind)?;

        Ok(())
    }

    fn write_code(&self, writer: &mut dyn WriteColor, error: &Span<'_>) -> Result<(), Error> {
        let mut output = ColorOutput::new(writer);
        let num_fmt = LineNumberFormatter::new(error);

        num_fmt.spaces(output.writer())?;
        writeln!(output.writer())?;

        for (line_number, line) in error.lines().numbered() {
            let line_fmt = LineFormatter::new(&line);

            num_fmt.number(output.writer(), line_number)?;
            line_fmt.render(output.writer())?;
            // currently, the span will always exist since we take the line from the error
            // but future versions may print a line below and above for context that
            // is not part of the error
            if let Some(faulty_part_of_line) = Span::intersect(error, &line) {
                // TODO: implement this without the following 3 assumptions:
                // - start_pos - end_pos >= 0, guranteed by data structure invariant of Span
                // - start_term_pos - end_term_pos >= 0, guranteed by monotony of columns
                //   (a Position.char() can only be rendered to 0 or more terminal characters)
                // - unwrap(.): both positions are guranteed to exist in the line since we just
                //   got them from the faulty line, which is a subset of the whole error line
                let (start_term_pos, end_term_pos) =
                    line_fmt.get_actual_columns(&faulty_part_of_line).unwrap();

                let term_width = end_term_pos - start_term_pos;

                num_fmt.spaces(output.writer())?;

                {
                    let mut output = ColorOutput::new(output.writer());
                    output.set_color(self.level.color());
                    output.set_bold(true);
                    writeln!(
                        output.writer(),
                        "{spaces}{underline}",
                        spaces = " ".repeat(start_term_pos),
                        underline = "^".repeat(term_width)
                    )?;
                }
            }
        }
        Ok(())
    }
}

/// Helper that prints a range of numbers with the correct
/// amount of padding
struct LineNumberFormatter {
    width: usize,
}

impl LineNumberFormatter {
    pub fn new(span: &Span<'_>) -> Self {
        Self {
            width: span.end_position().line_number().to_string().len(),
        }
    }

    pub fn spaces(&self, writer: &mut dyn WriteColor) -> Result<(), Error> {
        let mut output = ColorOutput::new(writer);
        output.set_color(HIGHLIGHT_COLOR);
        output.set_bold(true);
        write!(output.writer(), " {} | ", " ".repeat(self.width))?;
        Ok(())
    }

    pub fn number(&self, writer: &mut dyn WriteColor, line_number: usize) -> Result<(), Error> {
        let mut output = ColorOutput::new(writer);
        output.set_color(HIGHLIGHT_COLOR);
        output.set_bold(true);
        let padded_number = pad_left(&line_number.to_string(), self.width);
        write!(output.writer(), " {} | ", padded_number)?;
        Ok(())
    }
}

pub fn pad_left(s: &str, pad: usize) -> String {
    pad_left_with_char(s, pad, ' ')
}

pub fn pad_left_with_char(s: &str, pad: usize, chr: char) -> String {
    format!(
        "{padding}{string}",
        padding = chr
            .to_string()
            .repeat(pad.checked_sub(s.len()).unwrap_or(0)),
        string = s
    )
}

/// Writes a user-supplied input line in a safe manner by replacing
/// control-characters with escape sequences.
struct LineFormatter<'span, 'file> {
    line: &'span Span<'file>,
}

impl<'span, 'file> LineFormatter<'span, 'file> {
    fn new(line: &'span Span<'file>) -> Self {
        Self { line }
    }

    fn render(&self, writer: &mut dyn WriteColor) -> Result<(), Error> {
        let mut output = ColorOutput::new(writer);

        // TODO: implement an iterator
        let chars = self.line.start_position().iter();

        for position in chars {
            let (text, color) = self.render_char(position.chr());
            output.set_color(color);
            write!(output.writer(), "{}", text)?;

            if position == self.line.end_position() {
                break;
            }
        }

        writeln!(output.writer())?;

        Ok(())
    }

    /// Map terminal columns to `Position` columns. Returns a inclusive
    /// lower bound, and an exclusive upper bound.
    ///
    /// Each printed character does not actually take up monospace grid cell.
    /// For example a TAB character may be represented by 4 spaces. This
    /// function will return the actual number of 'monospace grid cells'
    /// rendered before the given
    /// position.
    ///
    /// Returns `None` if the column is out of bounds.
    fn get_actual_columns(&self, span: &Span<'_>) -> Option<(usize, usize)> {
        let lower = self.len_printed_before(span.start_position().column());
        let upper = self.len_printed_before(span.end_position().column());

        match (lower, upper) {
            (Some(lower), Some(upper)) => {
                let last_char_width = self.render_char(span.end_position().chr()).0.len();
                Some((lower, upper + last_char_width))
            }
            _ => None,
        }
    }

    fn len_printed_before(&self, col: usize) -> Option<usize> {
        // TODO: get rid of this nonsense
        // NOTE: it would actually be nice to condition the Position on the Line
        // instead of AsciiFile. Thinking of this, we could actually just do
        // `AsciiFile::new((span.as_str().as_bytes()))`. Meaning AsciiFile is
        // not a file, but a View
        // that restricts the
        // linked lists in Positions and Spans to a subset of the file.
        // TODO: implement an iterator on span, or
        // span.to_view().iter()/.to_ascii_file().iter() this method is
        // inherintly unsafe
        // because we do not have
        // a way to restrict
        // positions in a type safe manner.
        if self.line.len() < col {
            return None;
        }

        let chars = self.line.start_position().iter();

        let mut actual_column = 0;

        for position in chars {
            if position.column() == col {
                break;
            }

            actual_column += self.render_char(position.chr()).0.len();
        }

        Some(actual_column)
    }

    fn render_char(&self, chr: char) -> (String, Option<Color>) {
        match chr {
            '\t' => (" ".repeat(TAB_WIDTH), None),
            '\r' | '\n' => ("".to_string(), None),
            chr if chr.is_control() => (
                format!("{{{}}}", u8_to_printable_representation(chr as u8)),
                HIGHLIGHT_COLOR,
            ),
            _ => (chr.to_string(), None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pad_left() {
        let tests = vec![("a", "    a"), ("", "          "), ("a", "a"), ("", "")];

        for (input, expected) in tests {
            println!("Testing: {:?} => {:?}", input, expected);
            assert_eq!(expected, pad_left(input, expected.len()));
        }

        // not enough padding does not truncate string
        assert_eq!("a", pad_left("a", 0));
    }
}
