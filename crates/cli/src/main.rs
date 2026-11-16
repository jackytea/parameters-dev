use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use parameters_core::layout::complete_layouts;
use parameters_core::{from_json, to_json, validate};
use parameters_engine::builtins;

const USAGE: &str = "\
Usage: parameters <command> [options] <files>

Commands:
  check <files>...                  Report syntax and validation problems
  fmt [--check] <files>...          Rewrite files in canonical form; with --check, only report
  convert <file> [--output <file>]  Convert .parameters text to JSON, or JSON to text
  reference                         Print the language reference
  help                              Show this message
  version                           Show the version

Files ending in .json are read as JSON workflow documents. Other files are read as
Parameters text, usually with the .parameters extension.
";

enum Failure {
    Usage(String),
    Io(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    Text,
    Json,
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => code,
        Err(Failure::Usage(message)) => {
            eprintln!("error: {message}\n\n{USAGE}");
            ExitCode::from(2)
        }
        Err(Failure::Io(message)) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<ExitCode, Failure> {
    let Some((command, rest)) = args.split_first() else {
        return Err(Failure::Usage("missing command".into()));
    };
    match command.as_str() {
        "check" => check(&files(rest)?),
        "fmt" => {
            let (check_only, rest) = flag(rest, "--check");
            fmt(&files(&rest)?, check_only)
        }
        "convert" => convert(rest),
        "reference" => {
            print!("{}", parameters_lang::reference());
            Ok(ExitCode::SUCCESS)
        }
        "help" | "--help" | "-h" => {
            print!("{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        "version" | "--version" | "-V" => {
            println!("parameters {}", env!("CARGO_PKG_VERSION"));
            Ok(ExitCode::SUCCESS)
        }
        other => Err(Failure::Usage(format!("unknown command `{other}`"))),
    }
}

fn check(files: &[PathBuf]) -> Result<ExitCode, Failure> {
    let builtins = builtins();
    let mut errors = 0;
    let mut warnings = 0;
    for file in files {
        let source = read(file)?;
        let mut report = |is_error: bool, line: String| {
            if is_error {
                errors += 1;
            } else {
                warnings += 1;
            }
            eprintln!("{line}");
        };
        match format_of(file) {
            Format::Text => {
                for diagnostic in parameters_lang::check(&source, &builtins) {
                    report(
                        diagnostic.is_error(),
                        format!("{}:{diagnostic}", file.display()),
                    );
                }
            }
            Format::Json => match from_json(&source) {
                Ok(workflow) => {
                    for diagnostic in validate(&workflow, &builtins) {
                        report(
                            diagnostic.is_error(),
                            format!("{}: {diagnostic}", file.display()),
                        );
                    }
                }
                Err(error) => report(true, format!("{}: error: {error}", file.display())),
            },
        }
    }
    eprintln!(
        "checked {} {}: {}",
        files.len(),
        plural(files.len(), "file"),
        match (errors, warnings) {
            (0, 0) => "no problems".to_owned(),
            (errors, 0) => format!("{errors} {}", plural(errors, "error")),
            (0, warnings) => format!("{warnings} {}", plural(warnings, "warning")),
            (errors, warnings) => format!(
                "{errors} {}, {warnings} {}",
                plural(errors, "error"),
                plural(warnings, "warning")
            ),
        }
    );
    Ok(if errors > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn fmt(files: &[PathBuf], check_only: bool) -> Result<ExitCode, Failure> {
    let mut failed = false;
    for file in files {
        let source = read(file)?;
        let Some(formatted) = canonical(file, &source) else {
            failed = true;
            continue;
        };
        if formatted == source {
            continue;
        }
        if check_only {
            eprintln!("{} is not formatted", file.display());
            failed = true;
        } else {
            write(file, &formatted)?;
            eprintln!("formatted {}", file.display());
        }
    }
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn canonical(file: &Path, source: &str) -> Option<String> {
    match format_of(file) {
        Format::Text => match parameters_lang::format(source) {
            Ok(formatted) => Some(formatted),
            Err(diagnostics) => {
                for diagnostic in diagnostics {
                    eprintln!("{}:{diagnostic}", file.display());
                }
                None
            }
        },
        Format::Json => match from_json(source) {
            Ok(workflow) => Some(to_json(&workflow)),
            Err(error) => {
                eprintln!("{}: error: {error}", file.display());
                None
            }
        },
    }
}

fn convert(args: &[String]) -> Result<ExitCode, Failure> {
    let (output, rest) = option(args, &["--output", "-o"])?;
    let [input] = files(&rest)?
        .try_into()
        .map_err(|_| Failure::Usage("convert takes exactly one input file".into()))?;
    let source = read(&input)?;
    let converted = match format_of(&input) {
        Format::Text => {
            let parsed = parameters_lang::parse(&source);
            if parsed.has_errors() {
                for diagnostic in &parsed.diagnostics {
                    eprintln!("{}:{diagnostic}", input.display());
                }
                return Ok(ExitCode::FAILURE);
            }
            let mut workflow = parsed.workflow;
            complete_layouts(&mut workflow);
            to_json(&workflow)
        }
        Format::Json => match from_json(&source) {
            Ok(workflow) => parameters_lang::print(&workflow),
            Err(error) => {
                eprintln!("{}: error: {error}", input.display());
                return Ok(ExitCode::FAILURE);
            }
        },
    };
    match output {
        Some(path) => write(&path, &converted)?,
        None => print!("{converted}"),
    }
    Ok(ExitCode::SUCCESS)
}

fn files(args: &[String]) -> Result<Vec<PathBuf>, Failure> {
    if let Some(unknown) = args.iter().find(|arg| arg.starts_with('-')) {
        return Err(Failure::Usage(format!("unknown option `{unknown}`")));
    }
    if args.is_empty() {
        return Err(Failure::Usage("no files given".into()));
    }
    Ok(args.iter().map(PathBuf::from).collect())
}

fn flag(args: &[String], name: &str) -> (bool, Vec<String>) {
    let present = args.iter().any(|arg| arg == name);
    let rest = args.iter().filter(|arg| *arg != name).cloned().collect();
    (present, rest)
}

fn option(args: &[String], names: &[&str]) -> Result<(Option<PathBuf>, Vec<String>), Failure> {
    let mut value = None;
    let mut rest = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if names.contains(&arg.as_str()) {
            let next = iter
                .next()
                .ok_or_else(|| Failure::Usage(format!("`{arg}` needs a file name")))?;
            value = Some(PathBuf::from(next));
        } else {
            rest.push(arg.clone());
        }
    }
    Ok((value, rest))
}

fn format_of(path: &Path) -> Format {
    if path
        .extension()
        .is_some_and(|extension| extension == "json")
    {
        Format::Json
    } else {
        Format::Text
    }
}

fn read(path: &Path) -> Result<String, Failure> {
    fs::read_to_string(path)
        .map_err(|error| Failure::Io(format!("cannot read {}: {error}", path.display())))
}

fn write(path: &Path, contents: &str) -> Result<(), Failure> {
    fs::write(path, contents)
        .map_err(|error| Failure::Io(format!("cannot write {}: {error}", path.display())))
}

fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        noun.to_owned()
    } else {
        format!("{noun}s")
    }
}
