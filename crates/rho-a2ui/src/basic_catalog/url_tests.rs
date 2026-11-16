use serde_json::json;

use crate::{DataContext, DataModel, Effect, Expr, Functions};

/// The host learns which URL to open only through the effect, and a
/// refused URL asks for nothing.
#[test]
fn open_url_hands_the_url_to_the_host() {
    let model = DataModel::default();
    let functions = Functions::basic();
    let mut context = DataContext::new(&model, &functions);
    for url in ["https://example.com", "javascript:alert(1)"] {
        let call = Expr::from_json(&json!({"@call": "openUrl", "args": {"url": url}}))
            .expect("a valid call");
        let _ = context.evaluate(&call);
    }
    assert_eq!(
        context.take_effects(),
        [Effect::OpenUrl("https://example.com".to_owned())]
    );
}
