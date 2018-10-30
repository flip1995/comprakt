//! This is a list of semantic and lexical errors and warnings the compiler
//! emits.
//!
//! Error numbers could be generated automatically, _however_ error numbers
//! should be consistent for all versions of the compiler, even if errors or
//! warnings are retired.
//!
//! This implementation is NOT thread-safe.

// TODO: import spanned and span into this module?
use crate::{
    asciifile::AsciiFile,
    lexer::{Span, Spanned},
};
use failure::{AsFail, Fail};
use std::cell::RefCell;
use termcolor::{Color, ColorSpec, WriteColor};

// Error catalog.
//#[derive(Eq, PartialEq, Fail, Debug)]
//pub enum ErrorKind {
//}

//impl ErrorKind {
//fn get_message(&self) -> String {
//match self {
//ErrorKind::NonAsciiCharacter => {
//"encountered character outside of ASCII range, which is not
//"encountered allowed.".to_string()
//}
//ErrorKind::CommentSeparatorInsideComment => {
//"confusing usage of comment separator inside a comment.".to_string()
//}

//pub fn get_id(&self) -> String {
////format!("M{:03}", *self as u8)
//"E001".to_string()
//}

/// Tagging Interface marking failures as warnings.
/// Avoids accidental calls of error methods with warnings.
pub trait Warning: Fail {}

/// Tagging Interface marking failures as warnings.
/// Avoids accidental calls of error methods with warnings.
pub trait CompileError: Fail {}

/// Instead of writing errors, warnings and lints generated in the different
/// compiler stages directly to stdout, they are collected in this object.
///
/// This has several advantages:
/// - the output level can be adapted by users.
/// - we have a single source responsible for formatting compiler messages.
/// - unit tests can run the compiler and just assert the diagnostics object
///   instead of stdout/stderr of another process.
pub struct Diagnostics {
    // TODO: there is no reason to collect the messages except
    // for debugging purposes. So, maybe remove...
    messages: RefCell<Vec<Message>>,
    writer: RefCell<Box<dyn WriteColor>>,
}

impl Diagnostics {
    pub fn new(writer: Box<dyn WriteColor>) -> Self {
        Self {
            writer: RefCell::new(writer),
            messages: RefCell::new(Vec::new()),
        }
    }

    /// True when an error message was emitted, false
    /// if only warnings were emitted.
    pub fn errored(&self) -> bool {
        self.messages
            .borrow()
            .iter()
            .any(|msg| msg.level == MessageLevel::Error)
    }

    pub fn count(&self, level: MessageLevel) -> usize {
        self.messages
            .borrow()
            .iter()
            .filter(|msg| msg.level == level)
            .count()
    }

    fn write_statistics(&self) {
        let mut writer = self.writer.borrow_mut();

        if self.errored() {
            writer
                .set_color(ColorSpec::new().set_fg(MessageLevel::Error.color()))
                .ok();
            writeln!(
                writer,
                "Compilation aborted due to {}",
                match self.count(MessageLevel::Error) {
                    1 => "an error".to_string(),
                    n => format!("{} errors", n),
                }
            );
        } else {
            writer
                .set_color(ColorSpec::new().set_fg(Some(Color::Green)))
                .ok();
            writeln!(
                writer,
                "Compilation finished successfully {}",
                match self.count(MessageLevel::Warning) {
                    0 => "without warnings".to_string(),
                    1 => "with a warning".to_string(),
                    n => format!("with {} warnings", n),
                }
            );
        }
        writer.set_color(ColorSpec::new().set_fg(None)).ok();
    }

    // TODO: as we do not use warnings here. the warning trait is redundant!
    pub fn warning(&self, kind: Box<dyn AsFail>) {
        let msg = Message {
            level: MessageLevel::Warning,
            kind,
        };

        let mut writer = self.writer.borrow_mut();
        msg.write_colored(&mut **writer);
        &self.messages.borrow_mut().push(msg);
    }

    pub fn warning_with_source_snippet<'ctx>(
        &self,
        spanned: Spanned<Box<dyn AsFail>>,
        file: &AsciiFile<'ctx>,
    ) {
        let msg = Message {
            level: MessageLevel::Warning,
            kind: spanned.data,
        };

        let mut writer = self.writer.borrow_mut();
        msg.write_colored_with_code(&mut **writer, spanned.span, file);
        // TODO: store span
        &self.messages.borrow_mut().push(msg);
    }
}

#[derive(Copy, Clone, Eq, PartialEq)]
pub enum MessageLevel {
    Error,
    Warning,
}

impl MessageLevel {
    fn color(&self) -> Option<Color> {
        // Don't be confused by return type. `None` means default color!
        match self {
            MessageLevel::Error => Some(Color::Red),
            MessageLevel::Warning => Some(Color::Yellow),
        }
    }

    fn name(&self) -> &str {
        match self {
            MessageLevel::Error => "error",
            MessageLevel::Warning => "warning",
        }
    }
}

pub struct Message {
    pub level: MessageLevel,
    pub kind: Box<dyn AsFail>,
    /* TODO: draw code segment with error highlighted
     * pub span: Span,
     * TODO: maybe add suggestions for fixes
     * pub suggestions: Vec<Message>
     * TODO: filename seems unnecessary as we only compile a single file
     * pub filename: Path */
}

struct ColorOutput<'a> {
    writer: &'a mut dyn WriteColor,
    spec: ColorSpec,
}
impl<'a> ColorOutput<'a> {
    fn new(writer: &'a mut dyn WriteColor) -> Self {
        Self {
            writer,
            spec: ColorSpec::new(),
        }
    }

    fn set_color(&mut self, color: Option<Color>) {
        // ignore coloring failures using ok()
        self.spec.set_fg(color);
        self.writer.set_color(&self.spec).ok();
    }

    fn set_bold(&mut self, yes: bool) {
        // ignore coloring failures using ok()
        self.spec.set_bold(yes);
        self.writer.set_color(&self.spec).ok();
    }

    fn writer(&mut self) -> &mut dyn WriteColor {
        self.writer
    }
}

/// reset to no color by default. Otherwise code that
/// is not color aware will print everything in the
/// color last used.
impl<'a> Drop for ColorOutput<'a> {
    fn drop(&mut self) {
        // ignore coloring failures using ok()
        self.writer.reset().ok();
    }
}

impl Message {
    fn write_colored(&self, writer: &mut dyn WriteColor) {
        self.write_colored_header(writer);
        writeln!(writer, "");
    }

    fn write_colored_header(&self, writer: &mut dyn WriteColor) {
        let mut output = ColorOutput::new(writer);
        output.set_color(self.level.color());
        output.set_bold(true);
        write!(output.writer(), "{}: ", self.level.name());

        output.set_color(None);
        writeln!(output.writer(), "{}", self.kind.as_fail());
    }

    fn write_colored_with_code<'ctx>(
        &self,
        writer: &mut dyn WriteColor,
        span: Span,
        file: &AsciiFile<'ctx>,
    ) {
        self.write_colored_header(writer);

        let mut output = ColorOutput::new(writer);
        output.set_color(Some(Color::Cyan));
        output.set_bold(true);

        // TODO: pad with whitespace, right align
        let empty_line_marker = "     | ";
        writeln!(output.writer(), "{}", empty_line_marker);
        write!(output.writer(), "{:4} | ", span.start.row + 1);

        if span.is_multiline() {
            output.set_color(self.level.color());
            write!(output.writer(), ">");
        }

        output.set_bold(false);
        output.set_color(None);
        writeln!(output.writer(), "{}", span.start.get_line(file));

        output.set_color(Some(Color::Cyan));
        output.set_bold(true);
        write!(output.writer(), "{}", empty_line_marker);

        // TODO: print multiline spans correctly!
        if !span.is_multiline() {
            // add positional indicators.
            let indicator = format!(
                "{spaces}{markers}",
                spaces = " ".repeat(span.start.col + 1),
                markers = "^".repeat(span.end.col - span.start.col)
            );
            output.set_bold(true);
            output.set_color(self.level.color());
            writeln!(output.writer(), "{}", indicator);
        }
    }
}

/// Print a statistic at the end of compilation
impl Drop for Diagnostics {
    fn drop(&mut self) {
        self.write_statistics();
    }
}
