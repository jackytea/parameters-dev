use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};

use parameters_core::Workflow;
use parameters_engine::{CompileError, Engine, Plan};

const CAPACITY: usize = 128;

#[derive(Default)]
pub(crate) struct PlanCache {
    entries: Mutex<Entries>,
}

#[derive(Default)]
struct Entries {
    plans: HashMap<Vec<u8>, Arc<Plan>>,
    order: VecDeque<Vec<u8>>,
}

impl PlanCache {
    pub(crate) fn get_or_compile(
        &self,
        hash: &[u8],
        workflow: &Workflow,
        engine: &Engine,
    ) -> Result<Arc<Plan>, CompileError> {
        if let Some(plan) = self.lock().plans.get(hash) {
            return Ok(Arc::clone(plan));
        }
        let plan = Arc::new(engine.compile(workflow)?);
        let mut entries = self.lock();
        if !entries.plans.contains_key(hash) {
            if entries.order.len() >= CAPACITY
                && let Some(oldest) = entries.order.pop_front()
            {
                entries.plans.remove(&oldest);
            }
            entries.order.push_back(hash.to_vec());
            entries.plans.insert(hash.to_vec(), Arc::clone(&plan));
        }
        Ok(plan)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Entries> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
