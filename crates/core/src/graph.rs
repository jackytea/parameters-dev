use std::collections::{HashMap, VecDeque};

use crate::document::{FieldValue, Graph, Step};
use crate::expr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyGraph {
    dependency_offsets: Vec<usize>,
    dependencies: Vec<usize>,
    dependent_offsets: Vec<usize>,
    dependents: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cycles {
    pub cycles: Vec<Vec<usize>>,
}

impl DependencyGraph {
    #[must_use]
    pub fn of(graph: Graph<'_>) -> Self {
        let indices: HashMap<&str, usize> = graph
            .steps
            .iter()
            .enumerate()
            .map(|(index, step)| (step.id.as_str(), index))
            .collect();
        let dependencies: Vec<Vec<usize>> = graph
            .steps
            .iter()
            .map(|step| {
                step_dependency_names(step)
                    .into_iter()
                    .filter_map(|name| indices.get(name.as_str()).copied())
                    .collect()
            })
            .collect();
        Self::from_dependencies(&dependencies)
    }

    #[must_use]
    pub fn from_dependencies(dependencies: &[Vec<usize>]) -> Self {
        let count = dependencies.len();
        let mut dependency_offsets = Vec::with_capacity(count + 1);
        let mut flattened = Vec::new();
        let mut dependent_counts = vec![0; count];
        dependency_offsets.push(0);
        for list in dependencies {
            let mut unique: Vec<usize> = list
                .iter()
                .copied()
                .filter(|&index| index < count)
                .collect();
            unique.sort_unstable();
            unique.dedup();
            for &dependency in &unique {
                dependent_counts[dependency] += 1;
            }
            flattened.extend(unique);
            dependency_offsets.push(flattened.len());
        }
        let mut dependent_offsets = Vec::with_capacity(count + 1);
        dependent_offsets.push(0);
        for dependent_count in &dependent_counts {
            dependent_offsets
                .push(dependent_offsets[dependent_offsets.len() - 1] + dependent_count);
        }
        let mut cursor = dependent_offsets[..count].to_vec();
        let mut dependents = vec![0; flattened.len()];
        for step in 0..count {
            for &dependency in &flattened[dependency_offsets[step]..dependency_offsets[step + 1]] {
                dependents[cursor[dependency]] = step;
                cursor[dependency] += 1;
            }
        }
        Self {
            dependency_offsets,
            dependencies: flattened,
            dependent_offsets,
            dependents,
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.dependency_offsets.len() - 1
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[must_use]
    pub fn dependencies(&self, step: usize) -> &[usize] {
        &self.dependencies[self.dependency_offsets[step]..self.dependency_offsets[step + 1]]
    }

    #[must_use]
    pub fn dependents(&self, step: usize) -> &[usize] {
        &self.dependents[self.dependent_offsets[step]..self.dependent_offsets[step + 1]]
    }

    pub fn topological_order(&self) -> Result<Vec<usize>, Cycles> {
        let mut remaining: Vec<usize> = (0..self.len())
            .map(|step| self.dependencies(step).len())
            .collect();
        let mut ready: VecDeque<usize> = (0..self.len())
            .filter(|&step| remaining[step] == 0)
            .collect();
        let mut order = Vec::with_capacity(self.len());
        while let Some(step) = ready.pop_front() {
            order.push(step);
            for &dependent in self.dependents(step) {
                remaining[dependent] -= 1;
                if remaining[dependent] == 0 {
                    ready.push_back(dependent);
                }
            }
        }
        if order.len() == self.len() {
            Ok(order)
        } else {
            Err(Cycles {
                cycles: self.cycles(&remaining),
            })
        }
    }

    fn cycles(&self, remaining_dependencies: &[usize]) -> Vec<Vec<usize>> {
        let candidates: Vec<bool> = remaining_dependencies
            .iter()
            .map(|&count| count > 0)
            .collect();
        let finished = self.finishing_order(&candidates);
        let mut component_of = vec![usize::MAX; self.len()];
        let mut cycles = Vec::new();
        for &start in finished.iter().rev() {
            if component_of[start] != usize::MAX {
                continue;
            }
            let id = cycles.len();
            let mut members = Vec::new();
            let mut stack = vec![start];
            component_of[start] = id;
            while let Some(step) = stack.pop() {
                members.push(step);
                for &dependent in self.dependents(step) {
                    if candidates[dependent] && component_of[dependent] == usize::MAX {
                        component_of[dependent] = id;
                        stack.push(dependent);
                    }
                }
            }
            members.sort_unstable();
            cycles.push(members);
        }
        cycles.retain(|members| {
            members.len() > 1 || self.dependencies(members[0]).contains(&members[0])
        });
        cycles.sort_unstable();
        cycles
    }

    fn finishing_order(&self, candidates: &[bool]) -> Vec<usize> {
        let mut visited = vec![false; self.len()];
        let mut finished = Vec::new();
        for root in (0..self.len()).filter(|&step| candidates[step]) {
            if visited[root] {
                continue;
            }
            visited[root] = true;
            let mut stack = vec![(root, 0)];
            while let Some((step, next)) = stack.last_mut() {
                let dependencies = self.dependencies(*step);
                if let Some(&dependency) = dependencies.get(*next) {
                    *next += 1;
                    if candidates[dependency] && !visited[dependency] {
                        visited[dependency] = true;
                        stack.push((dependency, 0));
                    }
                } else {
                    finished.push(*step);
                    stack.pop();
                }
            }
        }
        finished
    }
}

#[must_use]
pub fn step_dependency_names(step: &Step) -> Vec<String> {
    let mut names = step.after.clone();
    let expressions = step
        .with
        .values()
        .filter_map(|value| match value {
            FieldValue::Expression(source) => Some(source.as_str()),
            FieldValue::Literal(_) => None,
        })
        .chain(step.when.as_deref());
    for source in expressions {
        if let Ok(parsed) = expr::parse(source) {
            names.extend(parsed.root_names().into_iter().map(str::to_owned));
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orders_dependencies_before_dependents() {
        let graph = DependencyGraph::from_dependencies(&[vec![], vec![0], vec![0], vec![1, 2, 1]]);
        assert_eq!(graph.dependencies(3), [1, 2]);
        assert_eq!(graph.dependents(0), [1, 2]);
        assert_eq!(graph.topological_order().unwrap(), [0, 1, 2, 3]);
    }

    #[test]
    fn reports_only_steps_on_cycles() {
        let graph =
            DependencyGraph::from_dependencies(&[vec![], vec![0, 3], vec![1], vec![2], vec![3]]);
        assert_eq!(
            graph.topological_order().unwrap_err().cycles,
            [vec![1, 2, 3]]
        );
        let self_loop = DependencyGraph::from_dependencies(&[vec![0]]);
        assert_eq!(self_loop.topological_order().unwrap_err().cycles, [vec![0]]);
    }

    #[test]
    fn separates_independent_cycles() {
        let graph = DependencyGraph::from_dependencies(&[
            vec![2],
            vec![0],
            vec![1],
            vec![3],
            vec![2, 5],
            vec![4],
        ]);
        assert_eq!(
            graph.topological_order().unwrap_err().cycles,
            [vec![0, 1, 2], vec![3], vec![4, 5]]
        );
    }

    #[test]
    fn collects_dependencies_from_after_fields_and_conditions() {
        let mut step = Step::new("notify", "http.request");
        step.after.push("build".into());
        step.with
            .insert("url", FieldValue::expression(r#""{deploy.url}/{site}""#));
        step.with.insert(
            "body",
            FieldValue::Literal(serde_json::json!({ "text": "{ignored}" })),
        );
        step.when = Some("test succeeded and lint succeeded".into());
        assert_eq!(
            step_dependency_names(&step),
            ["build", "deploy", "site", "test", "lint"]
        );
    }
}
