//! A surface: the components an agent placed on it, its data model, and the
//! catalog its components and calls default to.

use serde_json::Value;

use crate::component::Component;
use crate::data_model::{DataError, DataModel, WatchId};
use crate::message::Metadata;
use crate::pointer::Pointer;

/// One surface, as a [`MessageProcessor`](crate::MessageProcessor) holds it.
///
/// Its components change only through messages. Its data model also changes
/// when the user edits a bound input, through [`Surface::set_data`].
#[derive(Debug, Clone)]
pub struct Surface {
    pub(crate) id: String,
    pub(crate) default_catalog: Option<String>,
    pub(crate) send_data_model: bool,
    pub(crate) metadata: Option<Metadata>,
    pub(crate) data: DataModel,
    /// In the order they arrived; an update keeps a component's place.
    pub(crate) components: Vec<Component>,
}

impl Surface {
    /// The surface id.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The catalog components and calls that name none run in, if any.
    #[must_use]
    pub fn default_catalog(&self) -> Option<&str> {
        self.default_catalog.as_deref()
    }

    /// Whether every message to the agent carries this surface's data model.
    #[must_use]
    pub fn sends_data_model(&self) -> bool {
        self.send_data_model
    }

    /// The `createSurface` metadata.
    #[must_use]
    pub fn metadata(&self) -> Option<&Metadata> {
        self.metadata.as_ref()
    }

    /// The data model.
    #[must_use]
    pub fn data(&self) -> &DataModel {
        &self.data
    }

    /// The components, in the order they first arrived.
    #[must_use]
    pub fn components(&self) -> &[Component] {
        &self.components
    }

    /// The component `id`.
    #[must_use]
    pub fn component(&self, id: &str) -> Option<&Component> {
        self.components.iter().find(|component| component.id == id)
    }

    /// Writes `value` at `path`, as a two-way binding does when the user
    /// edits an input, and returns the watchers whose value changed.
    ///
    /// # Errors
    ///
    /// Fails as [`DataModel::set`] does, leaving the model unchanged.
    pub fn set_data(&mut self, path: &Pointer, value: Value) -> Result<Vec<WatchId>, DataError> {
        self.data.set(path, value)
    }

    /// Watches `path` in the data model; see [`DataModel::watch`].
    pub fn watch(&mut self, path: &Pointer) -> WatchId {
        self.data.watch(path)
    }

    /// Stops watching.
    pub fn unwatch(&mut self, id: WatchId) {
        self.data.unwatch(id);
    }
}
