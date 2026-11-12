//! Model catalog hydration.

use super::{
    background_tasks::{SessionOutput, TaskId},
    App,
};

impl App {
    pub(super) fn refresh_catalog_chrome(&mut self) {
        self.statusline.update_model(&self.info.runtime);
        self.statusline.refresh_reasoning_capabilities();
        if let Some(view) = self.exclusive.attach_view_mut() {
            view.refresh_reasoning_capabilities();
        }
    }

    /// Loads the models.dev catalog off the UI thread. Model chrome reads
    /// display names and reasoning capabilities from it, so its arrival
    /// refreshes that chrome.
    pub(super) fn start_catalog_hydrate(&mut self) {
        self.tasks.spawn(
            TaskId::CatalogHydrate,
            rho_providers::model::ensure_model_catalog_names(),
            |_| SessionOutput::CatalogHydrate.into(),
        );
    }
}
