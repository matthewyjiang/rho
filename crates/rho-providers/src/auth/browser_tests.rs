use super::{BrowserAvailability, BrowserEnvironment};

fn graphical() -> BrowserEnvironment {
    BrowserEnvironment {
        remote_shell: false,
        display_server: true,
        wsl_host: false,
        nested_harness: false,
    }
}

// Covers: headless vs graphical is decided only from injected facts
// Owner: auth browser policy
#[test]
fn resolve_is_conservative_and_table_driven() {
    let cases = [
        (
            "graphical local",
            graphical(),
            BrowserAvailability::Graphical,
        ),
        (
            "remote shell",
            BrowserEnvironment {
                remote_shell: true,
                ..graphical()
            },
            BrowserAvailability::Headless,
        ),
        (
            "nested harness",
            BrowserEnvironment {
                nested_harness: true,
                ..graphical()
            },
            BrowserAvailability::Headless,
        ),
        (
            "no display",
            BrowserEnvironment {
                display_server: false,
                ..graphical()
            },
            BrowserAvailability::Headless,
        ),
        (
            // Stock WSL often has no Linux display; the Windows host browser
            // still launches, so wsl_host alone is Graphical.
            "wsl without display",
            BrowserEnvironment {
                display_server: false,
                wsl_host: true,
                ..graphical()
            },
            BrowserAvailability::Graphical,
        ),
        (
            "remote wins over display",
            BrowserEnvironment {
                remote_shell: true,
                wsl_host: true,
                ..graphical()
            },
            BrowserAvailability::Headless,
        ),
    ];

    for (label, environment, expected) in cases {
        pretty_assertions::assert_eq!(
            BrowserAvailability::resolve(environment),
            expected,
            "{label}"
        );
    }
}
