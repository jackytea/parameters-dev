use std::process::ExitCode;
use std::time::{Duration, Instant};

use parameters_core::{FieldValue, Port, Signature, Step, ValueType, Workflow};
use parameters_engine::{
    BoxFuture, Engine, Inputs, Node, NodeContext, NodeError, Registry, RunControl, RunStatus,
    Sandbox, Values,
};
use serde_json::json;

const STEPS: usize = 1_000;
const ROUNDS: usize = 20;
const BUDGET_PER_STEP: Duration = Duration::from_micros(50);

struct Echo;

impl Node for Echo {
    fn description(&self) -> &'static str {
        "Returns its input."
    }

    fn signature(&self) -> Signature {
        Signature {
            inputs: vec![Port::optional("value", ValueType::Any)],
            outputs: vec![Port::optional("value", ValueType::Any)],
        }
    }

    fn run(&self, _: NodeContext, inputs: Inputs) -> BoxFuture<Result<Values, NodeError>> {
        let value = inputs.optional("value").cloned().unwrap_or_default();
        Box::pin(async move { Ok(Values::from_iter([("value".to_owned(), value)])) })
    }
}

fn workflow(chained: bool) -> Workflow {
    let mut workflow = Workflow::new("Benchmark");
    workflow.concurrency = Some(1_024);
    for index in 0..STEPS {
        let mut step = Step::new(format!("step_{index}"), "bench.echo");
        let value = if chained && index > 0 {
            FieldValue::expression(format!("step_{}.value", index - 1))
        } else {
            FieldValue::Literal(json!(index))
        };
        step.with.insert("value", value);
        workflow.steps.push(step);
    }
    workflow
}

fn main() -> ExitCode {
    let mut registry = Registry::empty();
    registry.register("bench.echo", Echo);
    let Ok(engine) = Engine::new(registry, std::env::temp_dir(), Sandbox::OPEN) else {
        eprintln!("cannot create the engine");
        return ExitCode::FAILURE;
    };
    let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    else {
        eprintln!("cannot start the runtime");
        return ExitCode::FAILURE;
    };
    let mut over_budget = false;
    println!("{STEPS} steps per run, {ROUNDS} runs (median per step, best per step)");
    for (name, chained) in [("independent", false), ("chained", true)] {
        let plan = match engine.compile(&workflow(chained)) {
            Ok(plan) => plan,
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::FAILURE;
            }
        };
        let mut timings: Vec<Duration> = (0..ROUNDS)
            .map(|_| {
                let started = Instant::now();
                let report =
                    runtime.block_on(engine.run(&plan, Values::new(), RunControl::default()));
                let elapsed = started.elapsed();
                assert_eq!(report.status, RunStatus::Succeeded, "{:?}", report.errors);
                elapsed
            })
            .collect();
        timings.sort_unstable();
        let steps = u32::try_from(STEPS).unwrap_or(u32::MAX);
        let median = timings[ROUNDS / 2] / steps;
        let best = timings[0] / steps;
        println!("  {name:<12} {median:>10.2?} {best:>10.2?}");
        over_budget |= median > BUDGET_PER_STEP;
    }
    println!("  budget per step: {BUDGET_PER_STEP:?}");
    if over_budget {
        eprintln!("the scheduler is over budget");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
