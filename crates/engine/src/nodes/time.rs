use parameters_core::{Duration, Port, Signature, ValueType};

use crate::node::{BoxFuture, Inputs, Node, NodeContext, NodeError, Values};

pub(crate) struct TimeWait;

impl Node for TimeWait {
    fn signature(&self) -> Signature {
        Signature {
            inputs: vec![Port::required("duration", ValueType::Text)],
            outputs: vec![],
        }
    }

    fn run(&self, _: NodeContext, inputs: Inputs) -> BoxFuture<Result<Values, NodeError>> {
        Box::pin(async move {
            let duration = Duration::parse(&inputs.text("duration")?).map_err(NodeError::new)?;
            tokio::time::sleep(duration.to_std()).await;
            Ok(Values::new())
        })
    }
}

pub(crate) struct LogWrite;

impl Node for LogWrite {
    fn signature(&self) -> Signature {
        Signature {
            inputs: vec![Port::required("message", ValueType::Text)],
            outputs: vec![],
        }
    }

    fn run(&self, context: NodeContext, inputs: Inputs) -> BoxFuture<Result<Values, NodeError>> {
        Box::pin(async move {
            context.log(&inputs.text("message")?).await;
            Ok(Values::new())
        })
    }
}
