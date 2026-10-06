use super::*;

// Covers: a pinned Cursor model missing from a non-empty cache warns at
// preflight and still launches.
// Owner: cursor session policy
#[test]
fn unknown_cached_model_warns() {
    use crate::cursor_runtime::models::{cache_models, CursorModel};
    use pretty_assertions::assert_eq;
    use rho_providers::model::provider_models::{
        with_provider_models_cache_dir_for_tests, CliProviderRefreshContext,
    };

    let cache = tempfile::tempdir().unwrap();
    with_provider_models_cache_dir_for_tests(cache.path().to_path_buf(), || {
        cache_models(
            &[CursorModel {
                id: "composer-2.5".into(),
                display_name: "Composer 2.5".into(),
                is_default: false,
                is_current: true,
                zdr: true,
            }],
            CliProviderRefreshContext::default(),
        )
        .unwrap();

        assert_eq!(
            unknown_cursor_model_warning(Some("not-a-cursor-model")),
            Some("cursor model 'not-a-cursor-model' is not in the cached list".into())
        );
        assert_eq!(
            unknown_cursor_model_warning(Some("composer-2.5[effort=high]")),
            None
        );
        assert_eq!(unknown_cursor_model_warning(None), None);
    });
}
