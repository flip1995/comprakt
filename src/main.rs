#![warn(rust_2018_idioms)]
#![feature(try_from)]
#![feature(if_while_or_patterns)]
#![feature(bind_by_move_pattern_guards)]
#![feature(const_str_as_bytes)]

use failure::{Error, Fail, ResultExt};
use memmap::Mmap;
use std::{fs::File, io, path::PathBuf, process::exit};
use structopt::StructOpt;

#[macro_use]
mod utils;
mod asciifile;
mod lexer;
mod strtab;
use self::lexer::TokenKind;

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
    #[fail(display = "failed to open MiniJava file {:?}", path)]
    OpenInput { path: PathBuf },
    #[fail(display = "failed to mmap MiniJava file {:?}", path)]
    Mmap { path: PathBuf },
    #[fail(display = "failed to decode MiniJava file: {:?}", path)]
    Ascii { path: PathBuf },
    #[fail(display = "failed to copy input file {:?} to stdout", input)]
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
}

fn main() {
    let cmd = CliCommand::from_args();

    if let Err(msg) = run_compiler(&cmd) {
        exit_with_error(&msg);
    }
}

fn run_compiler(cmd: &CliCommand) -> Result<(), Error> {
    match cmd {
        CliCommand::Echo { path } => {
            let mut f = File::open(&path).context(CliError::OpenInput { path: path.clone() })?;

            let mut stdout = io::stdout();
            io::copy(&mut f, &mut stdout).context(CliError::Echo {
                input: path.clone(),
            })?;
        }
        CliCommand::LexerTest { path } => {
            let file = File::open(&path).context(CliError::OpenInput { path: path.clone() })?;
            let mapping =
                (unsafe { Mmap::map(&file) }).context(CliError::Mmap { path: path.clone() })?;
            let ascii_file = asciifile::AsciiFile::new(mapping)
                .context(CliError::Ascii { path: path.clone() })?;

            let strtab = strtab::StringTable::new();
            let lexer = lexer::Lexer::new(ascii_file.iter(), &strtab);

            let mut stdout = io::stdout();
            // TOOD without vector? Maybe itertools::process_results? Or for-loop and
            // early-return?
            let tokens: Result<Vec<_>, _> = lexer.collect();
            run_lexer_test(tokens?.into_iter().map(|t| t.data), &mut stdout)?;
        }
    }

    Ok(())
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
    let mut causes = err.iter_chain();

    if let Some(err_msg) = causes.next() {
        writeln!(writer, "Error: {}", err_msg)?;
    } else {
        writeln!(writer, "Unknown Error")?;
    };

    for cause in causes {
        writeln!(writer, "    caused by: {}", cause)?;
    }

    Ok(())
}

fn run_lexer_test<L, O>(lexer: L, out: &mut O) -> Result<(), Error>
where
    L: Iterator<Item = TokenKind>,
    O: io::Write,
{
    let token_datas = lexer.filter(|token_data| match token_data {
        TokenKind::Whitespace | TokenKind::Comment(_) => false,
        _ => true,
    });

    for td in token_datas {
        writeln!(out, "{}", td)?;
    }

    Ok(())
}

#[cfg(test)]
mod lexertest_tests {

    macro_rules! lexer_test_with_tokens {
        ( $toks:expr ) => {{
            let v: Vec<TokenKind> = { $toks };
            let mut o = Vec::new();
            let res = run_lexer_test(v.into_iter(), &mut o);
            assert!(res.is_ok());
            String::from_utf8(o).expect("output mut be utf8")
        }};
    }

    use super::{
        lexer::{Keyword, Operator, TokenKind},
        run_lexer_test,
        strtab::StringTable,
    };

    #[test]
    fn newline_per_token() {
        let tokens = vec![
            TokenKind::Operator(Operator::Ampersand),
            TokenKind::Keyword(Keyword::Int),
        ];
        let tokens_len = tokens.len();
        let o = lexer_test_with_tokens![tokens];
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
            TokenKind::EOF,
        ];
        let o = lexer_test_with_tokens!(tokens);
        assert_eq!(o.lines().count(), 4);
        assert!(!o.contains("comment"));
        assert_eq!(&o, "&\ninteger literal foo\nif\nEOF\n")
    }

    #[test]
    fn keywords_as_is() {
        let tokens = vec![TokenKind::Keyword(Keyword::Float)];
        let o = lexer_test_with_tokens!(tokens);
        assert_eq!(&o, "float\n");
    }

    #[test]
    fn operators_as_is() {
        let o = lexer_test_with_tokens!(vec![TokenKind::Operator(Operator::Caret)]);
        assert_eq!(&o, "^\n");
    }

    #[test]
    fn ident_prefix() {
        let st = StringTable::new();
        let o = lexer_test_with_tokens!(vec![TokenKind::Identifier(st.intern("an_identifier"))]);
        assert_eq!(&o, "identifier an_identifier\n");
    }

    #[test]
    fn integer_literal_prefix() {
        let st = StringTable::new();
        let o = lexer_test_with_tokens!(vec![TokenKind::IntegerLiteral(st.intern("2342"))]);
        assert_eq!(&o, "integer literal 2342\n");
    }

    #[test]
    fn eof() {
        let o = lexer_test_with_tokens!(vec![TokenKind::EOF]);
        assert_eq!(&o, "EOF\n");
    }

}
