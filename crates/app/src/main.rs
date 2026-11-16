mod api;
mod failure;
#[cfg(feature = "server")]
mod server;

use dioxus::prelude::*;

const THEME: Asset = asset!("/assets/theme.css");

#[cfg(feature = "server")]
fn main() -> std::process::ExitCode {
    server::main()
}

#[cfg(not(feature = "server"))]
fn main() {
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    rsx! {
        document::Title { "Parameters" }
        document::Stylesheet { href: THEME }
        main { class: "shell",
            h1 { "Parameters" }
        }
    }
}
