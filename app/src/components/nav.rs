//! The bottom navigation bar and the tab it selects.

use dioxus::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Connect,
    Diagnostics,
    Gossip,
    Endpoints,
}

#[component]
pub fn Nav(current_tab: Signal<Tab>) -> Element {
    let active = current_tab();
    rsx! {
        nav { class: "nav",
            NavItem {
                label: "Connect",
                icon: "⇄",
                is_active: active == Tab::Connect,
                on_select: move |_| current_tab.clone().set(Tab::Connect),
            }
            NavItem {
                label: "Diagnostics",
                icon: "⌁",
                is_active: active == Tab::Diagnostics,
                on_select: move |_| current_tab.clone().set(Tab::Diagnostics),
            }
            NavItem {
                label: "Gossip",
                icon: "≈",
                is_active: active == Tab::Gossip,
                on_select: move |_| current_tab.clone().set(Tab::Gossip),
            }
            NavItem {
                label: "Endpoints",
                icon: "▣",
                is_active: active == Tab::Endpoints,
                on_select: move |_| current_tab.clone().set(Tab::Endpoints),
            }
        }
    }
}

#[component]
fn NavItem(label: String, icon: String, is_active: bool, on_select: EventHandler<()>) -> Element {
    let class = if is_active {
        "nav-item active"
    } else {
        "nav-item"
    };
    rsx! {
        button {
            class: "{class}",
            onclick: move |_| on_select.call(()),
            span { class: "nav-icon", "{icon}" }
            span { class: "nav-label", "{label}" }
        }
    }
}
