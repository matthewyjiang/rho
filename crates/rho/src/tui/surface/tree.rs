//! One surface read out of the A2UI processor: what to draw, and the
//! controls the keyboard reaches.
//!
//! The processor holds components by catalog name with untyped properties.
//! [`build`] reads them once into [`Node`]s, so drawing and input match on
//! types and never look a property up by name. A component or property
//! this module does not know draws as a placeholder.

use std::collections::{BTreeMap, HashMap};

use rho_a2ui::{
    resolve_path, Action, ChildRef, Expr, MessageProcessor, Pointer, Property, Resolved,
    ResolvedComponent, Scope, Severity,
};
use serde_json::Value;

/// How many components one surface draws. A template over a long list
/// draws a component per item, and a surface that large is cut off here.
const MAX_NODES: usize = 2_000;

/// How deep components nest before the rest is cut off.
const MAX_DEPTH: usize = 64;

/// A surface as it stands.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct SurfaceTree {
    pub(super) root: Node,
    /// The controls the keyboard reaches, in the order they are drawn.
    /// A [`Node`] names its control by its index here.
    pub(super) focusables: Vec<Focusable>,
}

/// A component instance: one component, in the scope it was resolved in.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct Target {
    pub(super) component: String,
    pub(super) scope: Scope,
}

/// A control the keyboard reaches.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Focusable {
    pub(super) target: Target,
    pub(super) control: Control,
}

/// What a control does, and what it writes to.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Control {
    /// Runs its action; `None` while one of its checks fails.
    Button {
        action: Option<Action>,
    },
    /// Shows or hides a modal's content.
    Modal,
    /// Switches among this many tabs.
    Tabs {
        count: usize,
    },
    CheckBox {
        binding: Pointer,
    },
    Field {
        binding: Pointer,
        kind: FieldKind,
    },
    /// One option of a choice picker, whose selection is a list of values
    /// at `binding`.
    Choice {
        binding: Pointer,
        value: String,
        multiple: bool,
    },
    Slider {
        binding: Pointer,
        min: f64,
        max: f64,
        step: f64,
    },
}

/// What a text field takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FieldKind {
    Short,
    Long,
    Number,
    Obscured,
    /// An ISO 8601 date, time, or both.
    DateTime {
        date: bool,
        time: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ButtonVariant {
    Default,
    Primary,
    Borderless,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MediaKind {
    Image,
    Video,
    Audio,
}

/// How much a `Text` weighs among the text around it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TextRole {
    Heading,
    Body,
    Caption,
}

/// A check that failed, as the surface shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Failure {
    pub(super) message: String,
    pub(super) blocks: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ChoiceOption {
    pub(super) label: String,
    pub(super) selected: bool,
    /// Its control, when the picker's value is bound.
    pub(super) focus: Option<usize>,
}

/// Something to draw. `focus` is the control's index in
/// [`SurfaceTree::focusables`]; an input whose value is not bound to the
/// data model has none, since nothing would keep what the user entered.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Node {
    Text {
        text: String,
        role: TextRole,
    },
    Icon(String),
    Media {
        kind: MediaKind,
        description: String,
        url: String,
    },
    Divider,
    Row(Vec<Node>),
    Column(Vec<Node>),
    Card(Box<Node>),
    Tabs {
        focus: usize,
        titles: Vec<String>,
        active: usize,
        body: Option<Box<Node>>,
    },
    Modal {
        focus: usize,
        label: String,
        /// The content, while the modal is open.
        content: Option<Box<Node>>,
    },
    Button {
        focus: usize,
        label: String,
        variant: ButtonVariant,
        failures: Vec<Failure>,
    },
    CheckBox {
        focus: Option<usize>,
        label: String,
        checked: bool,
        failures: Vec<Failure>,
    },
    Field {
        focus: Option<usize>,
        label: String,
        value: String,
        placeholder: String,
        kind: FieldKind,
        failures: Vec<Failure>,
    },
    Choices {
        label: String,
        options: Vec<ChoiceOption>,
        multiple: bool,
        chips: bool,
        failures: Vec<Failure>,
    },
    Slider {
        focus: Option<usize>,
        label: String,
        value: f64,
        min: f64,
        max: f64,
        failures: Vec<Failure>,
    },
    /// A component this client cannot draw, by its catalog name, or the
    /// place where a surface too large to draw was cut off.
    Placeholder(String),
}

/// What the user chose on a surface that its data model does not hold:
/// the tab showing, and whether a modal is open, by component.
pub(super) type LocalState = HashMap<Target, usize>;

/// Reads `surface` out of `processor`, or `None` when it has no such
/// surface or the surface has no `root` yet.
pub(super) fn build(
    processor: &MessageProcessor,
    surface: &str,
    local: &LocalState,
) -> Option<SurfaceTree> {
    let mut builder = Builder {
        processor,
        surface,
        local,
        focusables: Vec::new(),
        nodes: 0,
    };
    let root = ChildRef {
        id: "root".to_owned(),
        scope: Scope::root(),
    };
    processor.resolve(surface, &root.id, &root.scope)?;
    let root = builder.node(&root, 0);
    Some(SurfaceTree {
        root,
        focusables: builder.focusables,
    })
}

struct Builder<'a> {
    processor: &'a MessageProcessor,
    surface: &'a str,
    local: &'a LocalState,
    focusables: Vec<Focusable>,
    nodes: usize,
}

/// A resolved component's properties, read by name.
struct Properties<'a>(&'a BTreeMap<String, Resolved>);

impl<'a> Properties<'a> {
    fn text(&self, name: &str) -> String {
        self.0
            .get(name)
            .and_then(Resolved::text)
            .unwrap_or_default()
    }

    fn literal(&self, name: &str) -> Option<&'a str> {
        self.0.get(name)?.value()?.as_str()
    }

    fn number(&self, name: &str) -> Option<f64> {
        self.0.get(name).and_then(Resolved::number)
    }

    fn flag(&self, name: &str) -> bool {
        self.0
            .get(name)
            .and_then(Resolved::boolean)
            .unwrap_or(false)
    }

    fn child(&self, name: &str) -> Option<&'a ChildRef> {
        match self.0.get(name)? {
            Resolved::Child(child) => Some(child),
            _ => None,
        }
    }

    fn children(&self, name: &str) -> &'a [ChildRef] {
        match self.0.get(name) {
            Some(Resolved::Children(children)) => children,
            _ => &[],
        }
    }
}

impl Builder<'_> {
    fn node(&mut self, child: &ChildRef, depth: usize) -> Node {
        if self.nodes == MAX_NODES || depth == MAX_DEPTH {
            return Node::Placeholder("more".to_owned());
        }
        self.nodes += 1;
        let Some(resolved) = self
            .processor
            .resolve(self.surface, &child.id, &child.scope)
        else {
            return Node::Placeholder(child.id.clone());
        };
        let target = Target {
            component: child.id.clone(),
            scope: child.scope.clone(),
        };
        let properties = Properties(&resolved.properties);
        let failures = failures(&resolved);
        match resolved.kind.as_str() {
            "Text" => Node::Text {
                text: properties.text("text"),
                role: match properties.literal("variant") {
                    Some("h1" | "h2" | "h3" | "h4" | "h5") => TextRole::Heading,
                    Some("caption") => TextRole::Caption,
                    _ => TextRole::Body,
                },
            },
            "Icon" => Node::Icon(properties.literal("name").unwrap_or_default().to_owned()),
            "Image" => media(MediaKind::Image, &properties),
            "Video" => media(MediaKind::Video, &properties),
            "AudioPlayer" => media(MediaKind::Audio, &properties),
            "Divider" => Node::Divider,
            "Row" => Node::Row(self.nodes_of(properties.children("children"), depth)),
            "Column" => Node::Column(self.nodes_of(properties.children("children"), depth)),
            "List" => {
                let items = self.nodes_of(properties.children("children"), depth);
                if properties.literal("direction") == Some("horizontal") {
                    Node::Row(items)
                } else {
                    Node::Column(items)
                }
            }
            "Card" => match properties.child("child") {
                Some(child) => Node::Card(Box::new(self.node(child, depth + 1))),
                None => Node::Placeholder(resolved.kind),
            },
            "Tabs" => self.tabs(target, &resolved, depth),
            "Modal" => {
                let label = properties
                    .child("trigger")
                    .map(|trigger| self.label(trigger, depth + 1))
                    .unwrap_or_default();
                let open = self.local.get(&target).is_some_and(|open| *open != 0);
                let focus = self.focusable(target, Control::Modal);
                let content = properties
                    .child("content")
                    .filter(|_| open)
                    .map(|content| Box::new(self.node(content, depth + 1)));
                Node::Modal {
                    focus,
                    label,
                    content,
                }
            }
            "Button" => {
                let label = properties
                    .child("child")
                    .map(|child| self.label(child, depth + 1))
                    .unwrap_or_default();
                let action = match resolved.properties.get("action") {
                    Some(Resolved::Action(action)) if !blocked(&failures) => Some(action.clone()),
                    _ => None,
                };
                Node::Button {
                    focus: self.focusable(target, Control::Button { action }),
                    label,
                    variant: match properties.literal("variant") {
                        Some("primary") => ButtonVariant::Primary,
                        Some("borderless") => ButtonVariant::Borderless,
                        _ => ButtonVariant::Default,
                    },
                    failures,
                }
            }
            "CheckBox" => Node::CheckBox {
                focus: self
                    .binding(&target, "value")
                    .map(|binding| self.focusable(target, Control::CheckBox { binding })),
                label: properties.text("label"),
                checked: properties.flag("value"),
                failures,
            },
            "TextField" => {
                let kind = match properties.literal("variant") {
                    Some("longText") => FieldKind::Long,
                    Some("number") => FieldKind::Number,
                    Some("obscured") => FieldKind::Obscured,
                    _ => FieldKind::Short,
                };
                self.field(target, &properties, kind, failures)
            }
            "DateTimeInput" => {
                let kind = FieldKind::DateTime {
                    date: properties.flag("enableDate"),
                    time: properties.flag("enableTime"),
                };
                self.field(target, &properties, kind, failures)
            }
            "ChoicePicker" => self.choices(target, &properties, failures),
            "Slider" => {
                let min = properties.number("min").unwrap_or(0.0);
                let max = properties.number("max").unwrap_or(min).max(min);
                let step = match properties.number("steps") {
                    Some(steps) if steps >= 1.0 => (max - min) / steps,
                    _ if max - min > 20.0 => (max - min) / 20.0,
                    _ => 1.0,
                };
                Node::Slider {
                    focus: self.binding(&target, "value").map(|binding| {
                        self.focusable(
                            target,
                            Control::Slider {
                                binding,
                                min,
                                max,
                                step,
                            },
                        )
                    }),
                    label: properties.text("label"),
                    value: properties.number("value").unwrap_or(min).clamp(min, max),
                    min,
                    max,
                    failures,
                }
            }
            _ => Node::Placeholder(resolved.kind),
        }
    }

    fn nodes_of(&mut self, children: &[ChildRef], depth: usize) -> Vec<Node> {
        children
            .iter()
            .map(|child| self.node(child, depth + 1))
            .collect()
    }

    fn tabs(&mut self, target: Target, resolved: &ResolvedComponent, depth: usize) -> Node {
        let tabs: Vec<(String, Option<&ChildRef>)> = match resolved.properties.get("tabs") {
            Some(Resolved::List(tabs)) => tabs
                .iter()
                .filter_map(|tab| match tab {
                    Resolved::Object(tab) => {
                        let tab = Properties(tab);
                        Some((tab.text("title"), tab.child("child")))
                    }
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        let active = self
            .local
            .get(&target)
            .copied()
            .unwrap_or(0)
            .min(tabs.len().saturating_sub(1));
        let focus = self.focusable(target, Control::Tabs { count: tabs.len() });
        let body = tabs
            .get(active)
            .and_then(|(_, child)| *child)
            .map(|child| Box::new(self.node(child, depth + 1)));
        Node::Tabs {
            focus,
            titles: tabs.into_iter().map(|(title, _)| title).collect(),
            active,
            body,
        }
    }

    fn field(
        &mut self,
        target: Target,
        properties: &Properties<'_>,
        kind: FieldKind,
        failures: Vec<Failure>,
    ) -> Node {
        Node::Field {
            focus: self
                .binding(&target, "value")
                .map(|binding| self.focusable(target, Control::Field { binding, kind })),
            label: properties.text("label"),
            value: properties.text("value"),
            placeholder: properties.text("placeholder"),
            kind,
            failures,
        }
    }

    fn choices(
        &mut self,
        target: Target,
        properties: &Properties<'_>,
        failures: Vec<Failure>,
    ) -> Node {
        let multiple = properties.literal("variant") == Some("multipleSelection");
        let selected: Vec<&str> = properties
            .0
            .get("value")
            .and_then(Resolved::value)
            .and_then(Value::as_array)
            .map(|values| values.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let binding = self.binding(&target, "value");
        let options = properties
            .0
            .get("options")
            .and_then(Resolved::value)
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|option| {
                let value = option.get("value")?.as_str()?;
                let label = match option.get("label") {
                    Some(Value::String(label)) => label.clone(),
                    _ => value.to_owned(),
                };
                Some(ChoiceOption {
                    label,
                    selected: selected.contains(&value),
                    focus: binding.clone().map(|binding| {
                        self.focusable(
                            target.clone(),
                            Control::Choice {
                                binding,
                                value: value.to_owned(),
                                multiple,
                            },
                        )
                    }),
                })
            })
            .collect();
        Node::Choices {
            label: properties.text("label"),
            options,
            multiple,
            chips: properties.literal("displayStyle") == Some("chips"),
            failures,
        }
    }

    fn focusable(&mut self, target: Target, control: Control) -> usize {
        self.focusables.push(Focusable { target, control });
        self.focusables.len() - 1
    }

    /// Where in the data model `property` of `target` is bound, for a
    /// two-way binding to write what the user enters. A literal or a
    /// computed value has no such place.
    fn binding(&self, target: &Target, property: &str) -> Option<Pointer> {
        let component = self
            .processor
            .surface(self.surface)?
            .component(&target.component)?;
        let Property::Value(Expr::Path(path)) = component.properties().get(property)? else {
            return None;
        };
        Pointer::parse(&resolve_path(path, Some(target.scope.path()))).ok()
    }

    /// The text of the component at `child` and of everything under it, on
    /// one line: what a button says.
    fn label(&mut self, child: &ChildRef, depth: usize) -> String {
        let mut parts = Vec::new();
        self.label_parts(child, depth, &mut parts);
        parts.join(" ")
    }

    fn label_parts(&mut self, child: &ChildRef, depth: usize, parts: &mut Vec<String>) {
        if self.nodes == MAX_NODES || depth == MAX_DEPTH {
            return;
        }
        self.nodes += 1;
        let Some(resolved) = self
            .processor
            .resolve(self.surface, &child.id, &child.scope)
        else {
            return;
        };
        let properties = Properties(&resolved.properties);
        match resolved.kind.as_str() {
            "Text" => parts.push(properties.text("text")),
            "Icon" => parts.extend(
                properties
                    .literal("name")
                    .map(|name| super::render::icon_glyph(name).to_owned()),
            ),
            _ => {
                for property in resolved.properties.values() {
                    match property {
                        Resolved::Child(child) => self.label_parts(child, depth + 1, parts),
                        Resolved::Children(children) => {
                            for child in children {
                                self.label_parts(child, depth + 1, parts);
                            }
                        }
                        Resolved::Value(_)
                        | Resolved::Action(_)
                        | Resolved::List(_)
                        | Resolved::Object(_) => {}
                    }
                }
            }
        }
    }
}

fn media(kind: MediaKind, properties: &Properties<'_>) -> Node {
    Node::Media {
        kind,
        description: properties.text("description"),
        url: properties.text("url"),
    }
}

fn failures(resolved: &ResolvedComponent) -> Vec<Failure> {
    resolved
        .checks
        .iter()
        .flat_map(|checks| checks.failures())
        .map(|failure| Failure {
            message: failure
                .message
                .clone()
                .or_else(|| failure.code.clone())
                .unwrap_or_else(|| "invalid".to_owned()),
            blocks: failure.severity == Severity::Error,
        })
        .collect()
}

fn blocked(failures: &[Failure]) -> bool {
    failures.iter().any(|failure| failure.blocks)
}

#[cfg(test)]
#[path = "tree_tests.rs"]
mod tests;
