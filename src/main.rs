#![warn(rust_2018_idioms)]
#![feature(try_from)]
#![feature(if_while_or_patterns)]
#![feature(bind_by_move_pattern_guards)]
#![feature(const_str_as_bytes)]
#![feature(box_syntax)]
#![feature(repeat_generic_slice)]
#![feature(never_type)]
#![feature(nll)]
#![feature(core_intrinsics)]
#![feature(custom_attribute)]
#[macro_use]
extern crate derive_more;

use self::diagnostics::MaybeSpanned;
use failure::{Error, Fail, ResultExt};
use memmap::Mmap;
use std::{fs::File, io, path::PathBuf, process::exit};
use structopt::StructOpt;
use termcolor::{ColorChoice, StandardStream};

#[macro_use]
mod utils;
mod ast;
mod color;
mod context;
mod diagnostics;
mod lexer;
mod parser;
#[macro_use]
mod visitor;
mod print;
mod sem;
mod spantracker;
mod strtab;
use self::{
    context::Context,
    lexer::{Lexer, TokenKind},
    parser::Parser,
};
use asciifile::Spanned;

/// An error generated by the cli interface of the compiler
///
/// We apply the logic explained in [1] meaning in the current form it's hard
/// to destructure on the error type.
///
/// NOTE: this kind of error represents an exception inside the compiler. It
/// does NOT represent lexical or semantic issues of the MiniJava source code
/// given by the user via a command line call. See `Diagnostics` for errors and
/// warnings regarding MiniJava file contents.
///
/// ---
/// [1] https://rust-lang-nursery.github.io/failure/use-error.html
#[derive(Debug, Fail)]
enum CliError {
    #[fail(display = "cannot open input file {:?}", path)]
    OpenInput { path: PathBuf },
    #[fail(display = "cannot mmap input file {:?}", path)]
    Mmap { path: PathBuf },
    // TODO: this should not be a compiler error but a diagnostics error
    #[fail(display = "cannot decode input file {:?}", path)]
    Ascii { path: PathBuf },
    #[fail(display = "cannot copy input file {:?} to stdout", input)]
    Echo { input: PathBuf },
}

#[derive(StructOpt)]
#[structopt(name = "comprakt")]
enum CliCommand {
    #[structopt(name = "--echo")]
    /// Writes the input file to stdout without modification
    Echo {
        #[structopt(name = "FILE", parse(from_os_str))]
        path: PathBuf,
    },
    #[structopt(name = "--lextest")]
    /// Only run the lexer stage on the input file, write
    /// recognized tokens to stdout separated by newlines
    LexerTest {
        #[structopt(name = "FILE", parse(from_os_str))]
        path: PathBuf,
    },
    /// Only run the lexer and parser stages on the input file.
    #[structopt(name = "--parsetest")]
    ParserTest {
        #[structopt(name = "FILE", parse(from_os_str))]
        path: PathBuf,
    },
    #[structopt(name = "--print-ast")]
    PrintAst {
        #[structopt(name = "FILE", parse(from_os_str))]
        path: PathBuf,
    },
    #[structopt(name = "--debug-dumpast")] // nonstandard
    DebugDumpAst {
        #[structopt(name = "FILE", parse(from_os_str))]
        path: PathBuf,
    },
    #[structopt(name = "--check")]
    Check {
        #[structopt(name = "FILE", parse(from_os_str))]
        path: PathBuf,
    },
}

fn main() {
    let cmd = CliCommand::from_args();

    if let Err(msg) = run_compiler(&cmd) {
        exit_with_error(&msg);
    }
}

use crate::strtab::StringTable;

fn run_compiler(cmd: &CliCommand) -> Result<(), Error> {
    match cmd {
        CliCommand::Echo { path } => cmd_echo(path),
        CliCommand::LexerTest { path } => cmd_lextest(path),
        CliCommand::ParserTest { path } => cmd_parsetest(path),
        CliCommand::PrintAst { path } => cmd_printast(path, &print::pretty::print),
        CliCommand::DebugDumpAst { path } => cmd_printast(path, &print::structure::print),
        CliCommand::Check { path } => cmd_check(path, &sem::check),
    }
}

/// Print an error in a format intended for end users and terminate
/// the program.
fn exit_with_error(err: &Error) -> ! {
    let mut stderr = io::stderr();
    print_error(&mut stderr, err).expect("unable to print error");
    exit(1);
}

/// Print error objects in a format intended for end users
fn print_error(writer: &mut dyn io::Write, err: &Error) -> Result<(), Error> {
    writeln!(writer, "error: {}", err.as_fail())?;
    for cause in err.iter_causes() {
        writeln!(writer, "caused by: {}", cause)?;
    }
    Ok(())
}

fn cmd_echo(path: &PathBuf) -> Result<(), Error> {
    let mut f = File::open(&path).context(CliError::OpenInput { path: path.clone() })?;

    let mut stdout = io::stdout();
    io::copy(&mut f, &mut stdout).context(CliError::Echo {
        input: path.clone(),
    })?;

    Ok(())
}

macro_rules! setup_io {
    (let $context:ident = $path:expr) => {
        let path: &PathBuf = $path;
        let file = File::open(&path).context(CliError::OpenInput { path: path.clone() })?;
        let mmres = unsafe { Mmap::map(&file) };
        const EMPTY: [u8; 0] = [0; 0];
        let bytes: &[u8] = match mmres {
            Ok(ref m) => m,
            Err(e) if e.kind() == io::ErrorKind::InvalidInput => {
                // Linux returns EINVAL on file size 0, but let's be sure
                let file_size = file
                    .metadata()
                    .map(|m| m.len())
                    .context("could not get file metadata while interpreting mmap error")?;
                if file_size == 0 {
                    &EMPTY
                } else {
                    Err(e.context(CliError::Mmap { path: path.clone() }))?
                }
            }
            Err(e) => Err(e.context(CliError::Mmap { path: path.clone() }))?,
        };

        let ascii_file =
            asciifile::AsciiFile::new(&bytes).context(CliError::Ascii { path: path.clone() })?;

        let stderr = StandardStream::stderr(ColorChoice::Auto);
        let $context = Context::new(&ascii_file, box stderr);
    };
}

fn cmd_check<C>(path: &PathBuf, checker: &C) -> Result<(), Error>
where
    C: Fn(&ast::AST<'_>, &context::Context<'_>) -> Result<(), Error>,
{
    setup_io!(let context = path);
    let strtab = StringTable::new();
    let lexer = Lexer::new(&strtab, &context);

    // adapt lexer to fail on first error
    // filter whitespace and comments
    let unforgiving_lexer = lexer.filter_map(|result| match result {
        Ok(token) => match token.data {
            TokenKind::Whitespace | TokenKind::Comment(_) => None,
            _ => Some(token),
        },
        Err(lexical_error) => {
            context.error(Spanned {
                span: lexical_error.span,
                data: box lexical_error.data,
            });

            context.diagnostics.write_statistics();
            exit(1);
        }
    });

    let mut parser = Parser::new(unforgiving_lexer);

    let program = match parser.parse() {
        Ok(p) => p,
        Err(parser_error) => {
            // TODO: context.error should do this match automatically through
            // generic arguments
            match parser_error {
                MaybeSpanned::WithSpan(spanned) => context.error(Spanned {
                    span: spanned.span,
                    data: box spanned.data,
                }),
                MaybeSpanned::WithoutSpan(error) => context
                    .diagnostics
                    .error(MaybeSpanned::WithoutSpan(box error)),
            }

            context.diagnostics.write_statistics();
            exit(1);
        }
    };

    checker(&program, &context)
}

fn cmd_printast<P>(path: &PathBuf, printer: &P) -> Result<(), Error>
where
    P: Fn(&ast::AST<'_>, &mut dyn std::io::Write) -> Result<(), Error>,
{
    setup_io!(let context = path);
    let strtab = StringTable::new();
    let lexer = Lexer::new(&strtab, &context);

    // adapt lexer to fail on first error
    // filter whitespace and comments
    let unforgiving_lexer = lexer.filter_map(|result| match result {
        Ok(token) => match token.data {
            TokenKind::Whitespace | TokenKind::Comment(_) => None,
            _ => Some(token),
        },
        Err(lexical_error) => {
            context.error(Spanned {
                span: lexical_error.span,
                data: box lexical_error.data,
            });

            context.diagnostics.write_statistics();
            exit(1);
        }
    });

    let mut parser = Parser::new(unforgiving_lexer);

    let program = match parser.parse() {
        Ok(p) => p,
        Err(parser_error) => {
            // TODO: context.error should do this match automatically through
            // generic arguments
            match parser_error {
                MaybeSpanned::WithSpan(spanned) => context.error(Spanned {
                    span: spanned.span,
                    data: box spanned.data,
                }),
                MaybeSpanned::WithoutSpan(error) => context
                    .diagnostics
                    .error(MaybeSpanned::WithoutSpan(box error)),
            }

            context.diagnostics.write_statistics();
            exit(1);
        }
    };

    printer(&program, &mut std::io::stdout())
}

fn cmd_parsetest(path: &PathBuf) -> Result<(), Error> {
    setup_io!(let context = path);
    let strtab = StringTable::new();
    let lexer = Lexer::new(&strtab, &context);

    // adapt lexer to fail on first error
    // filter whitespace and comments
    let unforgiving_lexer = lexer.filter_map(|result| match result {
        Ok(token) => match token.data {
            TokenKind::Whitespace | TokenKind::Comment(_) => None,
            _ => Some(token),
        },
        Err(lexical_error) => {
            context.error(Spanned {
                span: lexical_error.span,
                data: box lexical_error.data,
            });

            context.diagnostics.write_statistics();
            exit(1);
        }
    });

    let mut parser = Parser::new(unforgiving_lexer);

    // TODO: implement diagnostics on parser
    //if context.diagnostics.errored() {
    //context.diagnostics.write_statistics();
    //exit(1);
    //}

    match parser.parse() {
        Ok(_) => Ok(()),
        Err(parser_error) => {
            // TODO: context.error should do this match automatically through
            // generic arguments
            match parser_error {
                MaybeSpanned::WithSpan(spanned) => {
                    context.diagnostics.error(MaybeSpanned::WithSpan(Spanned {
                        span: spanned.span,
                        data: box spanned.data,
                    }))
                }
                MaybeSpanned::WithoutSpan(error) => context
                    .diagnostics
                    .error(MaybeSpanned::WithoutSpan(box error)),
            }

            context.diagnostics.write_statistics();
            exit(1);
        }
    }
}

fn cmd_lextest(path: &PathBuf) -> Result<(), Error> {
    setup_io!(let context = path);
    let strtab = StringTable::new();
    let lexer = Lexer::new(&strtab, &context);

    let mut stdout = io::stdout();

    for token in lexer {
        match token {
            Err(lexical_error) => context.error(Spanned {
                span: lexical_error.span,
                data: box lexical_error.data,
            }),
            Ok(token) => write_token(&mut stdout, &token.data)?,
        }

        // stop compilation on first error during lexing phase
        if context.diagnostics.errored() {
            context.diagnostics.write_statistics();
            exit(1);
        }
    }

    write_eof_token(&mut stdout)?;

    Ok(())
}

fn write_token<O: io::Write>(out: &mut O, token: &TokenKind) -> Result<(), Error> {
    match token {
        TokenKind::Whitespace | TokenKind::Comment(_) => Ok(()),
        _ => {
            writeln!(out, "{}", token)?;
            Ok(())
        }
    }
}

fn write_eof_token<O: io::Write>(out: &mut O) -> Result<(), Error> {
    writeln!(out, "EOF")?;
    Ok(())
}

#[cfg(test)]
mod lexertest_tests {

    fn lexer_test_with_tokens(tokens: Vec<TokenKind>) -> String {
        let mut o = Vec::new();
        for token in tokens.into_iter() {
            let res = write_token(&mut o, &token);
            assert!(res.is_ok());
        }

        String::from_utf8(o).expect("output must be utf8")
    }

    use super::{
        lexer::{Keyword, Operator, TokenKind},
        strtab::StringTable,
        write_token,
    };

    #[test]
    fn newline_per_token() {
        let tokens = vec![
            TokenKind::Operator(Operator::Ampersand),
            TokenKind::Keyword(Keyword::Int),
        ];
        let tokens_len = tokens.len();
        let o = lexer_test_with_tokens(tokens);
        assert_eq!(o.lines().count(), tokens_len);
    }

    #[test]
    fn no_whitespace_and_comments() {
        let st = StringTable::new();
        let tokens = vec![
            TokenKind::Operator(Operator::Ampersand),
            TokenKind::Whitespace,
            TokenKind::IntegerLiteral(st.intern("foo")),
            TokenKind::Comment("comment".to_string()),
            TokenKind::Keyword(Keyword::If),
        ];
        let o = lexer_test_with_tokens(tokens);
        assert_eq!(o.lines().count(), 3);
        assert!(!o.contains("comment"));
        assert_eq!(&o, "&\ninteger literal foo\nif\n")
    }

    #[test]
    fn keywords_as_is() {
        let tokens = vec![TokenKind::Keyword(Keyword::Float)];
        let o = lexer_test_with_tokens(tokens);
        assert_eq!(&o, "float\n");
    }

    #[test]
    fn operators_as_is() {
        let o = lexer_test_with_tokens(vec![TokenKind::Operator(Operator::Caret)]);
        assert_eq!(&o, "^\n");
    }

    #[test]
    fn ident_prefix() {
        let st = StringTable::new();
        let o = lexer_test_with_tokens(vec![TokenKind::Identifier(st.intern("an_identifier"))]);
        assert_eq!(&o, "identifier an_identifier\n");
    }

    #[test]
    fn integer_literal_prefix() {
        let st = StringTable::new();
        let o = lexer_test_with_tokens(vec![TokenKind::IntegerLiteral(st.intern("2342"))]);
        assert_eq!(&o, "integer literal 2342\n");
    }
}
