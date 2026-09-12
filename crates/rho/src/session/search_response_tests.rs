use pretty_assertions::assert_eq;
use serde_json::Value;

use super::*;

fn group(text: &str) -> Group {
    Group {
        session: "session".into(),
        id: "id".into(),
        workspace: "/workspace".into(),
        matching_messages: 3,
        excerpts: vec![Excerpt {
            anchor: "message".into(),
            role: "user".into(),
            start: 0,
            end: text.chars().count(),
            total_chars: text.chars().count(),
            text: text.into(),
            omitted_blocks: 0,
        }],
        omitted_matches: 2,
    }
}

// Covers exact JSON budget accounting, including escapes, pagination digit
// changes and the reduction notice. Owner: session search page assembly.
#[test]
fn page_budget_accounts_for_wire_bytes_and_stops_at_the_first_rejected_group() {
    for (offset, text) in [(0, "plain"), (9, "\"\\\n\0é🦀"), (99, "longer evidence")] {
        let make_page = |budget| {
            Page::new(
                Context::new(Refresh::default()),
                Scope::All,
                offset,
                /*total_sessions*/ offset + 3,
                /*limit*/ 3,
                budget,
            )
        };
        let mut expected_page = make_page(usize::MAX);
        assert!(expected_page.push(group(text)).unwrap());
        let mut expected: Value = serde_json::from_str(&expected_page.finish().unwrap()).unwrap();
        // Find the exact budget including its own decimal representation.
        let mut budget = serde_json::to_vec(&expected).unwrap().len();
        loop {
            expected["output_budget_bytes"] = budget.into();
            let bytes = serde_json::to_vec(&expected).unwrap().len();
            if bytes == budget {
                break;
            }
            budget = bytes;
        }
        let mut page = make_page(budget);
        let mut fetched = 0;
        for candidate in (0..3).map(|_| {
            fetched += 1;
            group(text)
        }) {
            if !page.push(candidate).unwrap() {
                break;
            }
        }
        let output = page.finish().unwrap();
        assert_eq!(
            (
                fetched,
                output.len(),
                serde_json::from_str::<Value>(&output).unwrap()
            ),
            (2, budget, expected)
        );
        assert!(make_page(budget - 1).push(group(text)).is_err());
    }
}

// Covers accepting an exactly fitting complete page without charging for a
// reduction notice that will not be emitted. Owner: session search assembly.
#[test]
fn complete_pages_use_the_exact_budget_without_a_reduction_notice() {
    for (offset, total, limit, count) in [
        (0, 0, 3, 0),
        (0, 1, 100, 1),
        (9, 11, 1, 1),
        (9, 11, 2, 2),
        (99, 103, 2, 2),
    ] {
        let assemble = |budget| {
            let mut page = Page::new(
                Context::new(Refresh::default()),
                Scope::All,
                offset,
                total,
                limit,
                budget,
            );
            for _ in 0..count {
                assert!(page.push(group("\"🦀\n")).unwrap());
            }
            page.finish()
        };
        let expected = assemble(usize::MAX).unwrap();
        assert_eq!(assemble(expected.len()).unwrap(), expected);
    }
}
