//! A session's A2UI surfaces: what its `ui` calls showed, and what the user
//! did on them.
//!
//! One [`SurfaceHost`] lives as long as its session, across the tool sets
//! the session builds. It holds the same state a client's A2UI processor
//! builds from the session's [`rho_protocol::ToolContent::Surface`] items,
//! so a call is checked against the surfaces the user actually has.
//!
//! Nothing of it is saved. A surface's messages are a pure function of the
//! `ui` call that showed it, which the session's history keeps, so a
//! reload shows the calls again, in order ([`SurfaceHost::show`] from
//! replay). Showing is idempotent per call for that reason: replay and the
//! live tool may both show one call, and the second gets the first's
//! outcome.

use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, LazyLock, Mutex, MutexGuard},
};

use rho_a2ui::{
    parse_payload, Action, AgentMessage, AllowedMessage, Catalog, Express, Framing, Functions,
    MessageProcessor, PromptOptions, Property, Resolved, ResolvedComponent, Scope, Validation,
    ValidationConfig,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::{oneshot, Notify};

/// The messages a `ui` call may compile to. Calls to renderer functions are
/// left out: no client answers them yet.
const ALLOWED: [AllowedMessage; 4] = [
    AllowedMessage::CreateSurface,
    AllowedMessage::UpdateComponents,
    AllowedMessage::UpdateDataModel,
    AllowedMessage::DeleteSurface,
];

/// How many components one call resolves. A template over a long list
/// resolves a component per item, and a surface that large is checked only
/// this far.
const MAX_RESOLVED: usize = 2_000;

static EXPRESS: LazyLock<Express> = LazyLock::new(|| {
    Express::new(vec![Catalog::basic().clone()]).expect("the basic catalog compiles")
});

/// The Express prompt for the basic catalog, which the `ui` tool's
/// description teaches the notation with.
pub(super) fn express_prompt() -> String {
    EXPRESS
        .prompt(&PromptOptions {
            framing: Framing::Bare,
            allowed: Some(ALLOWED.to_vec()),
            examples: Vec::new(),
        })
        .expect("a prompt without examples has nothing to decompile")
}

/// What one `ui` call asks to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SurfaceRequest {
    /// The Express block.
    pub(crate) express: String,
    /// The block's default surface, or `None` for a new one.
    pub(crate) surface: Option<String>,
}

impl SurfaceRequest {
    /// The request of a saved `ui` call's arguments, or `None` when they
    /// are not a request, which the live call refused too.
    pub(crate) fn of_arguments(arguments: &Value) -> Option<Self> {
        super::Arguments::parse(arguments)
            .ok()
            .map(|arguments| arguments.request)
    }

    /// The block's default surface: the one asked for, or an id that
    /// depends only on the call, so a replay of the call names the same
    /// surface.
    fn surface_id(&self, call_id: &str) -> String {
        self.surface.clone().unwrap_or_else(|| {
            let mut hash = Sha256::new();
            hash.update(call_id.as_bytes());
            hash.update([0]);
            hash.update(self.express.as_bytes());
            let digest = hash.finalize();
            let short: String = digest[..4]
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            format!("ui-{short}")
        })
    }
}

/// What a `ui` call showed: checked A2UI v1.0 messages, all about one
/// surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SurfaceContent {
    pub(crate) surface: String,
    pub(crate) messages: Vec<Value>,
}

impl SurfaceContent {
    pub(crate) fn wire(&self) -> rho_protocol::ToolContent {
        rho_protocol::ToolContent::Surface {
            surface: self.surface.clone(),
            messages: self.messages.clone(),
        }
    }
}

/// Something the user did on a surface, as the model reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SurfaceAction {
    pub(crate) surface: String,
    /// The body of A2UI's `action` message, with the surface's data model
    /// as `dataModel` when the client sent one.
    pub(crate) action: Value,
}

/// A session's surfaces, shared by its `ui` tool, its replay and its host.
#[derive(Clone)]
pub(crate) struct SurfaceHost(Arc<Shared>);

struct Shared {
    state: Mutex<State>,
    /// Told when an action no call waits for arrives.
    acted: Arc<Notify>,
    /// Shared with every [`SurfaceViewer`], so its count says whether one
    /// lives.
    viewers: Arc<()>,
}

/// A client that draws the session's surfaces and follows the session, for
/// as long as this is kept: the user can act on a surface there. Whoever
/// holds the client's subscription holds this next to it.
#[derive(Debug)]
pub(crate) struct SurfaceViewer {
    _counted: Arc<()>,
}

/// A `ui` call's wait for the user's next action on its surface.
///
/// Dropping it ends the wait. An action [`SurfaceHost::act`] handed it
/// that the call did not take, as when its turn is cancelled in between,
/// goes back to the host, first in line for a turn to deliver.
pub(super) struct Waiting {
    host: SurfaceHost,
    action: oneshot::Receiver<SurfaceAction>,
}

impl Waiting {
    /// The user's next action on the surface. Fails when another call
    /// starts waiting on the surface, or the surfaces are reset.
    pub(super) async fn action(&mut self) -> Result<SurfaceAction, oneshot::error::RecvError> {
        (&mut self.action).await
    }
}

impl Drop for Waiting {
    fn drop(&mut self) {
        // Closed first, so `act` either sent before this and the action is
        // taken here, or fails to send and queues the action itself.
        self.action.close();
        if let Ok(action) = self.action.try_recv() {
            self.host.restore_actions(&[action]);
        }
    }
}

struct State {
    processor: MessageProcessor,
    /// What every call showed, in order: what a client's processor has
    /// applied, and what rebuilds `processor` after a call it refused.
    accepted: Vec<Arc<SurfaceContent>>,
    /// The latest call shown under each provider call id, and its outcome.
    shown: HashMap<String, (SurfaceRequest, Result<Arc<SurfaceContent>, String>)>,
    /// The call waiting on each surface.
    waiting: HashMap<String, oneshot::Sender<SurfaceAction>>,
    /// Actions no call waited for, until a turn delivers them.
    actions: VecDeque<SurfaceAction>,
}

impl std::fmt::Debug for SurfaceHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SurfaceHost").finish_non_exhaustive()
    }
}

impl Default for SurfaceHost {
    fn default() -> Self {
        Self(Arc::new(Shared {
            state: Mutex::new(State {
                processor: new_processor(),
                accepted: Vec::new(),
                shown: HashMap::new(),
                waiting: HashMap::new(),
                actions: VecDeque::new(),
            }),
            acted: Arc::new(Notify::new()),
            viewers: Arc::new(()),
        }))
    }
}

fn new_processor() -> MessageProcessor {
    let mut processor = MessageProcessor::new(Validation::Check(ValidationConfig::STRICT));
    processor
        .add_catalog(Catalog::basic().clone(), Functions::basic())
        .expect("the basic catalog compiles");
    processor
}

impl SurfaceHost {
    fn state(&self) -> MutexGuard<'_, State> {
        self.0
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Shows what call `call_id` asks for: compiles its block, applies the
    /// messages to the session's surfaces, and resolves every component of
    /// the surface once. The error is written for the model. A call it
    /// refuses changes nothing.
    ///
    /// Showing the same call again returns what it showed the first time.
    pub(crate) fn show(
        &self,
        call_id: &str,
        request: &SurfaceRequest,
    ) -> Result<Arc<SurfaceContent>, String> {
        self.state().show(call_id, request, Shows::Anything)
    }

    /// [`Self::show`], for a call that then waits for the user's next
    /// action on the surface. Showing and starting to wait are one step,
    /// so no action on the surface falls between them: a client that
    /// already shows the surface can send one at any moment.
    ///
    /// Refused, showing nothing, while no client that draws surfaces
    /// follows the session, since nobody could answer, and for a block
    /// that leaves no surface to act on.
    pub(super) fn show_waiting(
        &self,
        call_id: &str,
        request: &SurfaceRequest,
    ) -> Result<(Arc<SurfaceContent>, Waiting), String> {
        let mut state = self.state();
        if Arc::strong_count(&self.0.viewers) == 1 {
            return Err(
                "no client that draws surfaces is following the session, so nobody could act \
                 on one; ask in text, or show the surface without `wait`"
                    .to_owned(),
            );
        }
        let content = state.show(call_id, request, Shows::ToActOn)?;
        let (sender, action) = oneshot::channel();
        state.waiting.insert(content.surface.clone(), sender);
        let waiting = Waiting {
            host: self.clone(),
            action,
        };
        Ok((content, waiting))
    }

    /// Counts a client that draws surfaces among the session's followers
    /// until the returned [`SurfaceViewer`] drops.
    pub(crate) fn viewer(&self) -> SurfaceViewer {
        SurfaceViewer {
            _counted: Arc::clone(&self.0.viewers),
        }
    }

    /// Takes what the user did on `surface`: `message` is A2UI's `action`
    /// message, from a client. The call waiting on the surface gets it;
    /// with none, it waits for a turn to deliver it. The error says why the
    /// session has no use for the message.
    pub(crate) fn act(
        &self,
        surface: &str,
        message: &Value,
        data_model: Option<Value>,
    ) -> Result<(), String> {
        let mut state = self.state();
        let mut action = state.checked_action(surface, message)?;
        if let Some(data_model) = data_model {
            action.insert("dataModel".to_owned(), data_model);
        }
        let mut action = SurfaceAction {
            surface: surface.to_owned(),
            action: Value::Object(action),
        };
        if let Some(waiting) = state.waiting.remove(surface) {
            match waiting.send(action) {
                Ok(()) => return Ok(()),
                // Its call stopped waiting, as a cancelled turn's does.
                Err(unsent) => action = unsent,
            }
        }
        state.actions.push_back(action);
        drop(state);
        self.0.acted.notify_waiters();
        Ok(())
    }

    /// Whether an action waits for a turn to deliver it.
    pub(crate) fn has_action(&self) -> bool {
        !self.state().actions.is_empty()
    }

    /// Completes when an action no call waits for arrives. Subscribe
    /// before checking [`Self::has_action`], so one in between is not
    /// missed.
    pub(crate) fn acted_owned(&self) -> impl std::future::Future<Output = ()> + Send + 'static {
        Arc::clone(&self.0.acted).notified_owned()
    }

    /// Takes the actions that wait for a turn, oldest first.
    pub(crate) fn take_actions(&self) -> Vec<SurfaceAction> {
        self.state().actions.drain(..).collect()
    }

    /// Puts back actions a turn took and did not deliver, ahead of any
    /// that arrived since.
    pub(crate) fn restore_actions(&self, actions: &[SurfaceAction]) {
        if actions.is_empty() {
            return;
        }
        let mut state = self.state();
        for action in actions.iter().rev() {
            state.actions.push_front(action.clone());
        }
        drop(state);
        self.0.acted.notify_waiters();
    }

    /// Forgets every surface, waiting call and undelivered action: the
    /// session's conversation moved to another branch of its tree, whose
    /// calls replay shows next.
    pub(crate) fn reset(&self) {
        let mut state = self.state();
        state.processor = new_processor();
        state.accepted.clear();
        state.shown.clear();
        state.waiting.clear();
        state.actions.clear();
    }
}

/// What a call's block has to leave behind.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shows {
    /// Whatever it does, a block that deletes its surface included.
    Anything,
    /// A surface, which the call waits on.
    ToActOn,
}

impl State {
    fn show(
        &mut self,
        call_id: &str,
        request: &SurfaceRequest,
        shows: Shows,
    ) -> Result<Arc<SurfaceContent>, String> {
        if let Some((shown, outcome)) = self.shown.get(call_id) {
            if shown == request {
                return outcome.clone();
            }
        }
        let outcome = self.apply(call_id, request, shows);
        self.shown
            .insert(call_id.to_owned(), (request.clone(), outcome.clone()));
        outcome
    }

    fn apply(
        &mut self,
        call_id: &str,
        request: &SurfaceRequest,
        shows: Shows,
    ) -> Result<Arc<SurfaceContent>, String> {
        let surface = request.surface_id(call_id);
        let messages = EXPRESS
            .compile(&request.express, &surface)
            .map_err(|error| error.to_string())?;
        let surface = one_surface(&messages)?;
        let processed = self.processor.process(&Value::Array(messages.clone()));
        let refused = processed
            .errors()
            .map(|error| match error.path() {
                Some(path) if !path.is_empty() => format!("{} (at {path})", error.message()),
                _ => error.message().to_owned(),
            })
            .collect::<Vec<_>>();
        let refused = if !refused.is_empty() {
            Some(refused.join("; "))
        } else if shows == Shows::ToActOn && self.processor.surface(&surface).is_none() {
            Some(format!(
                "the block leaves no surface `{surface}`, so there is nothing to wait on; \
                 call again without `wait`"
            ))
        } else {
            self.unresolved(&surface)
        };
        if let Some(refused) = refused {
            // A message the processor took before the one it refused stays
            // applied, so start over from what the calls before showed.
            self.processor = new_processor();
            for content in &self.accepted {
                let _ = self
                    .processor
                    .process(&Value::Array(content.messages.clone()));
            }
            return Err(refused);
        }
        let content = Arc::new(SurfaceContent { surface, messages });
        self.accepted.push(Arc::clone(&content));
        Ok(content)
    }

    /// The first binding of `surface` that fails to resolve, walking from
    /// its root as a client draws it.
    fn unresolved(&self, surface: &str) -> Option<String> {
        let mut pending = vec![("root".to_owned(), Scope::root())];
        let mut resolved = 0;
        while let Some((id, scope)) = pending.pop() {
            if resolved == MAX_RESOLVED {
                break;
            }
            resolved += 1;
            let Some(ResolvedComponent {
                properties, errors, ..
            }) = self.processor.resolve(surface, &id, &scope)
            else {
                continue;
            };
            if let Some(error) = errors.first() {
                return Some(error.message().to_owned());
            }
            for property in properties.values() {
                children(property, &mut pending);
            }
        }
        None
    }

    /// The body of `message`, A2UI's `action` message for `surface`, once
    /// a component of the surface is found to send it.
    fn checked_action(
        &self,
        surface: &str,
        message: &Value,
    ) -> Result<serde_json::Map<String, Value>, String> {
        let Some(shown) = self.processor.surface(surface) else {
            return Err(format!("the session has no surface `{surface}`"));
        };
        let Some(action) = message.get("action").and_then(Value::as_object) else {
            return Err("`action` is not an A2UI `action` message".to_owned());
        };
        let text = |key: &str| action.get(key).and_then(Value::as_str);
        if text("surfaceId").is_some_and(|named| named != surface) {
            return Err(format!("the action is not about surface `{surface}`"));
        }
        let (Some(name), Some(source)) = (text("name"), text("sourceComponentId")) else {
            return Err("the action lacks its `name` or `sourceComponentId`".to_owned());
        };
        let Some(component) = shown.component(source) else {
            return Err(format!("surface `{surface}` has no component `{source}`"));
        };
        if !component
            .properties()
            .values()
            .any(|property| sends(property, name))
        {
            return Err(format!("component `{source}` has no action `{name}`"));
        }
        Ok(action.clone())
    }
}

/// The one surface `messages` are about.
fn one_surface(messages: &[Value]) -> Result<String, String> {
    let mut surface: Option<String> = None;
    for message in parse_payload(&Value::Array(messages.to_vec())) {
        let named = match message.map_err(|error| error.message().to_owned())? {
            AgentMessage::CreateSurface(create) => create.surface_id,
            AgentMessage::UpdateComponents(update) => update.surface_id,
            AgentMessage::UpdateDataModel(update) => update.surface_id,
            AgentMessage::DeleteSurface { surface_id } => surface_id,
            AgentMessage::CallRendererFunction(_) | AgentMessage::AgentFunctionResponse(_) => {
                return Err("function calls are not available; show a surface instead".to_owned())
            }
        };
        match &surface {
            Some(first) if *first != named => {
                return Err(format!(
                    "one call shows one surface, and this block names `{first}` and `{named}`; \
                     use a call for each"
                ))
            }
            Some(_) => {}
            None => surface = Some(named),
        }
    }
    surface.ok_or_else(|| "the block does nothing".to_owned())
}

/// Queues the children `property` holds, each with the scope to resolve
/// it in.
fn children(property: &Resolved, pending: &mut Vec<(String, Scope)>) {
    match property {
        Resolved::Child(child) => pending.push((child.id.clone(), child.scope.clone())),
        Resolved::Children(all) => {
            pending.extend(
                all.iter()
                    .map(|child| (child.id.clone(), child.scope.clone())),
            );
        }
        Resolved::List(items) => items.iter().for_each(|item| children(item, pending)),
        Resolved::Object(members) => members.values().for_each(|item| children(item, pending)),
        Resolved::Value(_) | Resolved::Action(_) => {}
    }
}

/// Whether `property` holds an action that sends event `name` to the agent.
fn sends(property: &Property, name: &str) -> bool {
    match property {
        Property::Action(Action::Event(event)) => event.name == name,
        Property::List(items) => items.iter().any(|item| sends(item, name)),
        Property::Object(members) => members.values().any(|item| sends(item, name)),
        Property::Action(Action::Call(_))
        | Property::Value(_)
        | Property::Child(_)
        | Property::Children(_) => false,
    }
}

#[cfg(test)]
#[path = "host_tests.rs"]
mod tests;
