use std::collections::BTreeMap;
use std::fmt::Write;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use parameters_core::{Port, Signature, ValueType, validate};
use parameters_lang::{parse, print};

const STEPS: usize = 1_000;
const ROUNDS: usize = 50;
const BUDGET: Duration = Duration::from_millis(5);

fn main() -> ExitCode {
    let source = document();
    let catalog = BTreeMap::from([(
        "shell.run".to_owned(),
        Signature {
            inputs: vec![Port::required("command", ValueType::Text)],
            outputs: vec![Port::optional("stdout", ValueType::Text)],
        },
    )]);
    let check = measure(|| {
        let parsed = parse(&source);
        let diagnostics = validate(&parsed.workflow, &catalog);
        !parsed.has_errors() && diagnostics.is_empty()
    });
    let parsing = measure(|| !parse(&source).has_errors());
    let workflow = parse(&source).workflow;
    let validating = measure(|| validate(&workflow, &catalog).is_empty());
    let printing = measure(|| print(&workflow) == source);
    println!("{STEPS} steps, {} bytes (median, best)", source.len());
    for (name, (median, best)) in [
        ("parse + validate", check),
        ("parse", parsing),
        ("validate", validating),
        ("print", printing),
    ] {
        println!("  {name:<17} {median:>12.3?} {best:>12.3?}");
    }
    println!("  budget for parse + validate: {BUDGET:?}");
    if check.0 > BUDGET {
        eprintln!("parse + validate is over budget");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn measure(mut run: impl FnMut() -> bool) -> (Duration, Duration) {
    let mut timings: Vec<Duration> = (0..ROUNDS)
        .map(|_| {
            let start = Instant::now();
            let correct = run();
            let elapsed = start.elapsed();
            assert!(
                correct,
                "the benchmark document should be valid and canonical"
            );
            elapsed
        })
        .collect();
    timings.sort_unstable();
    (timings[ROUNDS / 2], timings[0])
}

fn document() -> String {
    let mut source = String::from("workflow \"Benchmark\"\n");
    for index in 0..STEPS {
        let _ = write!(source, "\nstep step_{index} uses shell.run\n");
        if index > 0 {
            let previous = index - 1;
            let _ = writeln!(source, "  after step_{previous}");
            let _ = writeln!(
                source,
                "  command is \"echo {{step_{previous}.stdout}} and more text\""
            );
        } else {
            source.push_str("  command is \"echo start\"\n");
        }
    }
    source
}
