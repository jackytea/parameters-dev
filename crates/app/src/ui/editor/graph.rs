use std::rc::Rc;

use dioxus::html::geometry::ClientPoint;
use dioxus::prelude::*;
use parameters_core::edit::{add_dependency, add_step, remove_dependency};
use parameters_core::{DependencyGraph, Position};

use crate::ui::editor::EditorState;

const NODE_WIDTH: f64 = 200.0;
const NODE_HEIGHT: f64 = 64.0;
const MIN_ZOOM: f64 = 0.25;
const MAX_ZOOM: f64 = 2.0;
const TOOLBAR: f64 = 52.0;
const MARGIN: f64 = 24.0;
const START: (f64, f64) = (MARGIN, TOOLBAR + MARGIN);

#[derive(Debug, Clone, Copy, PartialEq)]
struct View {
    x: f64,
    y: f64,
    zoom: f64,
}

impl Default for View {
    fn default() -> Self {
        Self {
            x: START.0,
            y: START.1,
            zoom: 1.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Drag {
    Pan {
        start: ClientPoint,
        origin: View,
    },
    Move {
        id: String,
        start: ClientPoint,
        from: Position,
    },
    Connect {
        from: String,
        start: ClientPoint,
        anchor: (f64, f64),
        pointer: (f64, f64),
    },
}

#[derive(Debug, Clone, PartialEq)]
struct Edge {
    from: String,
    to: String,
    path: String,
    explicit: bool,
}

fn curve(from: (f64, f64), to: (f64, f64)) -> String {
    let bend = ((to.0 - from.0).abs() / 2.0).max(40.0);
    format!(
        "M {} {} C {} {}, {} {}, {} {}",
        from.0,
        from.1,
        from.0 + bend,
        from.1,
        to.0 - bend,
        to.1,
        to.0,
        to.1
    )
}

fn bounds(points: &[(f64, f64)]) -> Option<(f64, f64, f64, f64)> {
    points.iter().fold(None, |bounds, &(x, y)| {
        let (left, top, right, bottom) = bounds.unwrap_or((x, y, x + NODE_WIDTH, y + NODE_HEIGHT));
        Some((
            left.min(x),
            top.min(y),
            right.max(x + NODE_WIDTH),
            bottom.max(y + NODE_HEIGHT),
        ))
    })
}

async fn fitted(canvas: Option<Rc<MountedData>>, points: Vec<(f64, f64)>) -> Option<View> {
    let rect = canvas?.get_client_rect().await.ok()?;
    let (left, top, right, bottom) = bounds(&points)?;
    let width = rect.size.width - 2.0 * MARGIN;
    let height = rect.size.height - TOOLBAR - 2.0 * MARGIN;
    let zoom = (width / (right - left))
        .min(height / (bottom - top))
        .clamp(MIN_ZOOM, 1.0);
    Some(View {
        x: MARGIN + (width - (right - left) * zoom).max(0.0) / 2.0 - left * zoom,
        y: TOOLBAR + MARGIN - top * zoom,
        zoom,
    })
}

fn round(value: f64) -> i32 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "canvas coordinates are clamped far inside the i32 range"
    )]
    let rounded = value.clamp(-1_000_000.0, 1_000_000.0).round() as i32;
    rounded
}

#[component]
pub(crate) fn GraphCanvas() -> Element {
    let state = use_context::<EditorState>();
    let mut view = use_signal(View::default);
    let mut drag = use_signal(|| None::<Drag>);
    let mut adding = use_signal(String::new);
    let mut canvas = use_signal(|| None::<Rc<MountedData>>);
    let mut fitted_once = use_signal(|| false);
    let points = move || {
        let workflow = state.workflow.peek();
        workflow
            .steps
            .iter()
            .map(|step| {
                workflow
                    .layout
                    .get(&step.id)
                    .map_or((0.0, 0.0), |position| {
                        (f64::from(position.x), f64::from(position.y))
                    })
            })
            .collect::<Vec<_>>()
    };
    let fit = move || async move {
        if let Some(next) = fitted(canvas.peek().clone(), points()).await {
            view.set(next);
        }
    };
    use_effect(move || {
        let ready = canvas.read().is_some() && !state.workflow.read().steps.is_empty();
        if ready && !fitted_once() {
            fitted_once.set(true);
            spawn(fit());
        }
    });

    let workflow = state.workflow.read();
    let editable = *state.text_valid.read();
    let selected = state.selected.read().clone();
    let live = state.live.read();
    let position = |id: &str| {
        workflow.layout.get(id).map_or((0.0, 0.0), |position| {
            (f64::from(position.x), f64::from(position.y))
        })
    };
    let graph = DependencyGraph::of(workflow.graph());
    let mut edges = Vec::new();
    for (index, step) in workflow.steps.iter().enumerate() {
        for &dependency in graph.dependencies(index) {
            let before = &workflow.steps[dependency];
            let (fx, fy) = position(&before.id);
            let (tx, ty) = position(&step.id);
            edges.push(Edge {
                from: before.id.clone(),
                to: step.id.clone(),
                path: curve(
                    (fx + NODE_WIDTH, fy + NODE_HEIGHT / 2.0),
                    (tx, ty + NODE_HEIGHT / 2.0),
                ),
                explicit: step.after.contains(&before.id),
            });
        }
    }
    let pending = match &*drag.read() {
        Some(Drag::Connect {
            anchor, pointer, ..
        }) => Some(curve(*anchor, *pointer)),
        _ => None,
    };
    let current = view();
    let nodes: Vec<_> = workflow
        .steps
        .iter()
        .map(|step| {
            let (x, y) = position(&step.id);
            let status = live
                .steps
                .get(&step.id)
                .map(|record| record.status.name())
                .unwrap_or_default();
            (step.id.clone(), step.uses.clone(), x, y, status)
        })
        .collect();
    let types: Vec<String> = state
        .catalog
        .read()
        .nodes()
        .iter()
        .map(|node| node.type_name.clone())
        .collect();
    drop(workflow);
    drop(live);

    let on_move = move |event: PointerEvent| {
        let point = event.client_coordinates();
        let Some(active) = drag.read().clone() else {
            return;
        };
        match active {
            Drag::Pan { start, origin } => view.set(View {
                x: origin.x + point.x - start.x,
                y: origin.y + point.y - start.y,
                zoom: origin.zoom,
            }),
            Drag::Move { id, start, from } => {
                let zoom = view.peek().zoom;
                state.place(
                    &id,
                    Position::new(
                        from.x + round((point.x - start.x) / zoom),
                        from.y + round((point.y - start.y) / zoom),
                    ),
                );
            }
            Drag::Connect {
                from,
                start,
                anchor,
                ..
            } => {
                let zoom = view.peek().zoom;
                drag.set(Some(Drag::Connect {
                    from,
                    start,
                    anchor,
                    pointer: (
                        anchor.0 + (point.x - start.x) / zoom,
                        anchor.1 + (point.y - start.y) / zoom,
                    ),
                }));
            }
        }
    };

    rsx! {
        div {
            class: if editable { "canvas" } else { "canvas readonly" },
            onmounted: move |event| canvas.set(Some(event.data())),
            onpointerdown: move |event| {
                drag.set(Some(Drag::Pan {
                    start: event.client_coordinates(),
                    origin: view(),
                }));
            },
            onpointermove: on_move,
            onpointerup: move |_| drag.set(None),
            onpointerleave: move |_| drag.set(None),
            onwheel: move |event| {
                event.prevent_default();
                let delta = event.delta().strip_units().y;
                let mut next = view();
                next.zoom = (next.zoom * (1.0 - delta * 0.001)).clamp(MIN_ZOOM, MAX_ZOOM);
                view.set(next);
            },
            div { class: "canvas-tools", onpointerdown: move |event| event.stop_propagation(),
                select {
                    "aria-label": "Node to add",
                    value: "{adding}",
                    disabled: !editable,
                    onchange: move |event| adding.set(event.value()),
                    option { value: "", "Add a step…" }
                    for node_type in types {
                        option { key: "{node_type}", value: "{node_type}", "{node_type}" }
                    }
                }
                button {
                    disabled: !editable || adding.read().is_empty(),
                    onclick: move |_| {
                        let node_type = adding.peek().clone();
                        let mut added = String::new();
                        state.edit(|workflow| {
                            added = add_step(workflow, &node_type, None);
                            Ok(())
                        });
                        if !added.is_empty() {
                            let mut selected = state.selected;
                            selected.set(Some(added));
                        }
                        adding.set(String::new());
                    },
                    "Add"
                }
                button { title: "Zoom out", onclick: move |_| view.with_mut(|view| view.zoom = (view.zoom / 1.2).max(MIN_ZOOM)), "−" }
                button { title: "Zoom in", onclick: move |_| view.with_mut(|view| view.zoom = (view.zoom * 1.2).min(MAX_ZOOM)), "+" }
                button { title: "Show every step", onclick: move |_| fit(), "Fit" }
            }
            div {
                class: "layer",
                style: "transform: translate({current.x}px, {current.y}px) scale({current.zoom});",
                svg { class: "edges", width: "1", height: "1",
                    for edge in edges {
                        path {
                            key: "{edge.from}-{edge.to}",
                            d: "{edge.path}",
                            class: if edge.explicit { "edge explicit" } else { "edge" },
                            onpointerdown: move |event| event.stop_propagation(),
                            onclick: move |_| {
                                if edge.explicit {
                                    let (from, to) = (edge.from.clone(), edge.to.clone());
                                    state.edit(|workflow| {
                                        remove_dependency(workflow, &from, &to);
                                        Ok(())
                                    });
                                }
                            },
                            title { if edge.explicit { "{edge.to} waits for {edge.from}. Click to remove." } else { "{edge.to} uses a value from {edge.from}" } }
                        }
                    }
                    if let Some(path) = pending {
                        path { d: "{path}", class: "edge pending" }
                    }
                }
                for (id , uses , x , y , status) in nodes {
                    GraphNode {
                        key: "{id}",
                        id: id.clone(),
                        uses,
                        x,
                        y,
                        status,
                        selected: selected.as_deref() == Some(id.as_str()),
                        editable,
                        drag,
                    }
                }
            }
            if state.workflow.read().steps.is_empty() {
                p { class: "canvas-empty", "Add a step to start building, or write the workflow as text." }
            }
        }
    }
}

#[component]
fn GraphNode(
    id: String,
    uses: String,
    x: f64,
    y: f64,
    status: String,
    selected: bool,
    editable: bool,
    drag: Signal<Option<Drag>>,
) -> Element {
    let mut state = use_context::<EditorState>();
    let mut class = String::from("node");
    if selected {
        class.push_str(" selected");
    }
    if !status.is_empty() {
        class.push_str(" status-");
        class.push_str(&status);
    }
    let down_id = id.clone();
    let up_id = id.clone();
    let port_id = id.clone();
    rsx! {
        div {
            class,
            style: "left: {x}px; top: {y}px; width: {NODE_WIDTH}px; height: {NODE_HEIGHT}px;",
            tabindex: "0",
            role: "button",
            "aria-label": "Step {id}, uses {uses}",
            onpointerdown: move |event| {
                event.stop_propagation();
                state.selected.set(Some(down_id.clone()));
                if editable {
                    let workflow = state.workflow.peek();
                    let from = workflow.layout.get(&down_id).copied().unwrap_or_default();
                    drag.set(Some(Drag::Move {
                        id: down_id.clone(),
                        start: event.client_coordinates(),
                        from,
                    }));
                }
            },
            onpointerup: move |event| {
                let active = drag.peek().clone();
                if let Some(Drag::Connect { from, .. }) = active {
                    event.stop_propagation();
                    drag.set(None);
                    if from != up_id {
                        let target = up_id.clone();
                        state.edit(|workflow| add_dependency(workflow, &from, &target).map_err(|error| error.to_string()));
                    }
                }
            },
            onkeydown: move |event| {
                if event.key() == Key::Enter {
                    state.selected.set(Some(id.clone()));
                }
            },
            div { class: "port in", "aria-hidden": "true" }
            div { class: "node-title", "{id}" }
            div { class: "node-type", "{uses}" }
            if !status.is_empty() {
                div { class: "node-status", "{status}" }
            }
            if editable {
                div {
                    class: "port out",
                    title: "Drag to another step to make it wait for this one",
                    onpointerdown: move |event| {
                        event.stop_propagation();
                        drag.set(Some(Drag::Connect {
                            from: port_id.clone(),
                            start: event.client_coordinates(),
                            anchor: (x + NODE_WIDTH, y + NODE_HEIGHT / 2.0),
                            pointer: (x + NODE_WIDTH, y + NODE_HEIGHT / 2.0),
                        }));
                    },
                }
            }
        }
    }
}
