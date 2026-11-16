mod api;
mod catalog;
mod failure;
#[cfg(feature = "server")]
mod server;
mod ui;

use dioxus::prelude::*;
use parameters_api::Uuid;

use crate::ui::editor::Editor;
use crate::ui::nodes::Nodes;
use crate::ui::runs::{RunPage, Runs};
use crate::ui::secrets::Secrets;
use crate::ui::workflows::Workflows;
use crate::ui::{NotFound, Shell};

const THEME: Asset = asset!("/assets/theme.css");

#[cfg(feature = "server")]
fn main() -> std::process::ExitCode {
    server::main()
}

#[cfg(not(feature = "server"))]
fn main() {
    dioxus::launch(App);
}

#[derive(Debug, Clone, PartialEq, Routable)]
#[rustfmt::skip]
enum Route {
    #[layout(Shell)]
        #[route("/")]
        Workflows {},
        #[route("/workflows/:id")]
        Editor { id: Uuid },
        #[route("/runs")]
        Runs {},
        #[route("/runs/:id")]
        RunPage { id: Uuid },
        #[route("/nodes")]
        Nodes {},
        #[route("/secrets")]
        Secrets {},
        #[route("/:..segments")]
        NotFound { segments: Vec<String> },
}

#[component]
fn App() -> Element {
    rsx! {
        document::Title { "Parameters" }
        document::Stylesheet { href: THEME }
        Router::<Route> {}
    }
}
