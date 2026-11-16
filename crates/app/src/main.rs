use dioxus::prelude::*;

const THEME: Asset = asset!("/assets/theme.css");

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
