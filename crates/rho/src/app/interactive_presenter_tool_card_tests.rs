use pretty_assertions::assert_eq;
use rho_tools::tool_card::{ToolBody, ToolFamily, ToolHeader, ToolStatus};

use super::*;

// Covers: attach cards retain typed code syntax while old/plain cards keep the
// exact existing wire shape. Owner: Rho presentation attachment contract.
#[test]
fn body_syntax_round_trips_without_changing_plain_card_wire_shape() {
    let card = ToolCard::new(
        ToolStatus::Running,
        ToolFamily::Default,
        ToolHeader::call("example", None),
    )
    .with_body(ToolBody::Lines(vec!["value = 1".into()]));
    let plain: PresentedToolCard = card.clone().into();
    assert_eq!(
        serde_json::to_vec(&plain).unwrap(),
        serde_json::to_vec(&card).unwrap()
    );
    for body_syntax in [
        ToolBodySyntax::Plain,
        ToolBodySyntax::Code {
            language: "python".into(),
            window: ToolBodyWindow::Head,
        },
        ToolBodySyntax::Code {
            language: "python".into(),
            window: ToolBodyWindow::Tail,
        },
    ] {
        let presented = PresentedToolCard {
            card: card.clone(),
            body_syntax,
        };
        let wire = serde_json::to_value(&presented).unwrap();
        assert_eq!(
            serde_json::from_value::<PresentedToolCard>(wire.clone()).unwrap(),
            presented
        );
        // Legacy readers ignore the additive syntax field and retain the body.
        assert_eq!(serde_json::from_value::<ToolCard>(wire).unwrap(), card);
    }
}
