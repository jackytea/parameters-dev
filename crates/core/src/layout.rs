use std::collections::{HashMap, HashSet};

use crate::document::{Graph, Position, Step, Workflow};
use crate::graph::DependencyGraph;
use crate::ordered_map::OrderedMap;

pub const COLUMN_WIDTH: i32 = 280;
pub const ROW_HEIGHT: i32 = 120;
const ORDERING_SWEEPS: usize = 4;

#[must_use]
pub fn auto_layout(graph: &DependencyGraph) -> Vec<Position> {
    let order = graph
        .topological_order()
        .unwrap_or_else(|_| (0..graph.len()).collect());
    let mut layer_of = vec![0; graph.len()];
    for &step in &order {
        layer_of[step] = graph
            .dependencies(step)
            .iter()
            .map(|&dependency| layer_of[dependency] + 1)
            .max()
            .unwrap_or(0);
    }
    let layer_count = layer_of.iter().max().map_or(0, |&deepest| deepest + 1);
    let mut layers: Vec<Vec<usize>> = vec![Vec::new(); layer_count];
    for step in 0..graph.len() {
        layers[layer_of[step]].push(step);
    }
    let mut row_of = vec![0.0; graph.len()];
    renumber(&layers, &mut row_of);
    for _ in 0..ORDERING_SWEEPS {
        for layer in layers.iter_mut().skip(1) {
            sort_by_barycenter(layer, &row_of, |step| graph.dependencies(step));
        }
        renumber(&layers, &mut row_of);
        for layer in layers.iter_mut().rev().skip(1) {
            sort_by_barycenter(layer, &row_of, |step| graph.dependents(step));
        }
        renumber(&layers, &mut row_of);
    }
    let mut positions = vec![Position::default(); graph.len()];
    for (column, layer) in layers.iter().enumerate() {
        for (row, &step) in layer.iter().enumerate() {
            positions[step] = Position::new(scaled(column, COLUMN_WIDTH), scaled(row, ROW_HEIGHT));
        }
    }
    positions
}

pub fn complete_layouts(workflow: &mut Workflow) {
    for graph in workflow.graphs_mut() {
        complete_layout(graph.steps, graph.layout);
    }
}

pub fn complete_layout(steps: &[Step], layout: &mut OrderedMap<Position>) {
    let dependencies = DependencyGraph::of(Graph {
        inputs: &[],
        steps,
        outputs: &[],
        layout,
    });
    let mut placed: Vec<Option<Position>> = {
        let existing: HashMap<&str, Position> = layout
            .iter()
            .map(|(id, position)| (id, *position))
            .collect();
        steps
            .iter()
            .map(|step| existing.get(step.id.as_str()).copied())
            .collect()
    };
    let mut seen = HashSet::new();
    let mut place = |layout: &mut OrderedMap<Position>, step: usize, position: Position| {
        if seen.insert(steps[step].id.as_str()) {
            layout.push_absent(steps[step].id.clone(), position);
        }
    };
    if placed.iter().all(Option::is_none) {
        for (step, position) in auto_layout(&dependencies).into_iter().enumerate() {
            place(layout, step, position);
        }
        return;
    }
    let mut lowest_in_column: HashMap<i32, i32> = HashMap::new();
    for position in placed.iter().flatten() {
        let lowest = lowest_in_column.entry(position.x).or_insert(position.y);
        *lowest = (*lowest).max(position.y);
    }
    let order = dependencies
        .topological_order()
        .unwrap_or_else(|_| (0..steps.len()).collect());
    for step in order {
        if placed[step].is_some() {
            continue;
        }
        let x = dependencies
            .dependencies(step)
            .iter()
            .filter_map(|&dependency| placed[dependency])
            .map(|position| position.x + COLUMN_WIDTH)
            .max()
            .unwrap_or(0);
        let y = lowest_in_column
            .get(&x)
            .map_or(0, |lowest| lowest + ROW_HEIGHT);
        let position = Position::new(x, y);
        lowest_in_column.insert(x, y);
        placed[step] = Some(position);
        place(layout, step, position);
    }
}

fn sort_by_barycenter<'a>(
    layer: &mut [usize],
    row_of: &[f64],
    neighbours: impl Fn(usize) -> &'a [usize],
) {
    let barycenter = |step: usize| {
        let adjacent = neighbours(step);
        if adjacent.is_empty() {
            row_of[step]
        } else {
            adjacent
                .iter()
                .map(|&neighbour| row_of[neighbour])
                .sum::<f64>()
                / count(adjacent.len())
        }
    };
    let mut keyed: Vec<(f64, usize)> = layer.iter().map(|&step| (barycenter(step), step)).collect();
    keyed.sort_by(|left, right| left.0.total_cmp(&right.0));
    for (slot, (_, step)) in layer.iter_mut().zip(keyed) {
        *slot = step;
    }
}

fn renumber(layers: &[Vec<usize>], row_of: &mut [f64]) {
    for layer in layers {
        for (row, &step) in layer.iter().enumerate() {
            row_of[step] = count(row);
        }
    }
}

fn count(value: usize) -> f64 {
    f64::from(u32::try_from(value).unwrap_or(u32::MAX))
}

fn scaled(index: usize, unit: i32) -> i32 {
    i32::try_from(index).map_or(i32::MAX, |index| index.saturating_mul(unit))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steps(dependencies: &[(&str, &[&str])]) -> Vec<Step> {
        dependencies
            .iter()
            .map(|(id, after)| {
                let mut step = Step::new(*id, "shell.run");
                step.after = after.iter().map(|name| (*name).to_owned()).collect();
                step
            })
            .collect()
    }

    #[test]
    fn places_parallel_steps_in_one_column_and_dependents_to_the_right() {
        let steps = steps(&[
            ("checkout", &[]),
            ("test", &["checkout"]),
            ("lint", &["checkout"]),
            ("deploy", &["test", "lint"]),
        ]);
        let mut layout = OrderedMap::new();
        complete_layout(&steps, &mut layout);
        let at = |id: &str| *layout.get(id).unwrap();
        assert_eq!(at("checkout"), Position::new(0, 0));
        assert_eq!(at("test"), Position::new(280, 0));
        assert_eq!(at("lint"), Position::new(280, 120));
        assert_eq!(at("deploy"), Position::new(560, 0));
    }

    #[test]
    fn barycenter_ordering_removes_crossings() {
        let graph = DependencyGraph::from_dependencies(&[vec![], vec![], vec![1], vec![0]]);
        let positions = auto_layout(&graph);
        assert_eq!(positions[2].y, positions[1].y);
        assert_eq!(positions[3].y, positions[0].y);
    }

    #[test]
    fn keeps_existing_positions_and_places_new_steps_beside_their_dependencies() {
        let steps = steps(&[
            ("build", &[]),
            ("deploy", &["build"]),
            ("notify", &["deploy"]),
        ]);
        let mut layout = OrderedMap::new();
        layout.insert("build", Position::new(40, 40));
        layout.insert("deploy", Position::new(320, 40));
        complete_layout(&steps, &mut layout);
        assert_eq!(layout.get("build"), Some(&Position::new(40, 40)));
        assert_eq!(layout.get("notify"), Some(&Position::new(600, 0)));
        assert_eq!(layout.len(), 3);
    }

    #[test]
    fn lays_out_cyclic_graphs_without_panicking() {
        let graph = DependencyGraph::from_dependencies(&[vec![1], vec![0]]);
        assert_eq!(auto_layout(&graph).len(), 2);
    }
}
