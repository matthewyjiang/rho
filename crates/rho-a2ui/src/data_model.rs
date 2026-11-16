use serde_json::{Map, Value};
use thiserror::Error;

use crate::pointer::Pointer;

/// The largest list index a path may address, a bound from the conformance
/// suite that keeps one write from allocating without limit.
pub const MAX_LIST_INDEX: usize = 10_000;

/// A handle to a watcher registered with [`DataModel::watch`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WatchId(u64);

/// Why a write to a [`DataModel`] failed. The model is unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DataError {
    /// The path runs through a value that is neither an object, a list nor null.
    #[error("cannot set {pointer}: {at} holds a primitive")]
    PrimitiveInPath {
        /// The path that was written.
        pointer: Pointer,
        /// The path of the primitive that is in the way.
        at: Pointer,
    },
    /// The path addresses a list element with a segment that is not an index.
    #[error("cannot set {pointer}: non-numeric segment {segment:?} addresses a list")]
    NonNumericSegment {
        /// The path that was written.
        pointer: Pointer,
        /// The segment that is not a list index.
        segment: String,
    },
    /// The path addresses a list element above [`MAX_LIST_INDEX`].
    #[error("cannot set {pointer}: list index {segment} is above {MAX_LIST_INDEX}")]
    ListIndexTooLarge {
        /// The path that was written.
        pointer: Pointer,
        /// The segment that is too large.
        segment: String,
    },
}

/// How a path segment reads as a list index.
enum Index {
    NotNumeric,
    Valid(usize),
    TooLarge,
}

impl Index {
    /// ASCII digits with no leading zero, except `0` itself.
    fn of(segment: &str) -> Self {
        let digits = !segment.is_empty() && segment.bytes().all(|b| b.is_ascii_digit());
        if !digits || (segment.len() > 1 && segment.starts_with('0')) {
            return Self::NotNumeric;
        }
        match segment.parse::<usize>() {
            Ok(index) if index <= MAX_LIST_INDEX => Self::Valid(index),
            _ => Self::TooLarge,
        }
    }
}

/// The value tree behind a surface, with pull-based change watchers.
///
/// `Value::Null` written to a path removes it, as `updateDataModel` does on
/// the wire. Watchers record a path; [`DataModel::set`] returns the ones whose
/// value changed, and the caller reads the new value with [`DataModel::get`].
#[derive(Debug, Clone)]
pub struct DataModel {
    root: Value,
    watchers: Vec<(WatchId, Pointer)>,
    next_watch: u64,
}

impl Default for DataModel {
    /// An empty object.
    fn default() -> Self {
        Self::new(Value::Object(Map::new()))
    }
}

impl DataModel {
    /// A model holding `root`, which may be any value.
    #[must_use]
    pub fn new(root: Value) -> Self {
        Self {
            root,
            watchers: Vec::new(),
            next_watch: 0,
        }
    }

    /// The value at `pointer`, or `None` when nothing is there.
    ///
    /// A segment that is not a valid list index addresses nothing in a list.
    #[must_use]
    pub fn get(&self, pointer: &Pointer) -> Option<&Value> {
        pointer
            .segments()
            .iter()
            .try_fold(&self.root, |node, segment| child(node, segment))
    }

    /// Writes `value` at `pointer` and returns the watchers whose value
    /// changed, in registration order.
    ///
    /// `Value::Null` removes: an object key is deleted, a list element in
    /// range becomes null, and a path with nothing there is left alone. Null
    /// at the root resets the model to `{}`. Missing parents are created, as
    /// lists when the next segment is an index and as objects otherwise.
    /// Writing past the end of a list pads it with null.
    ///
    /// # Errors
    ///
    /// Fails, leaving the model unchanged, when the path runs through a
    /// primitive, or addresses a list with a segment that is not a valid index.
    pub fn set(&mut self, pointer: &Pointer, value: Value) -> Result<Vec<WatchId>, DataError> {
        let affected: Vec<usize> = (0..self.watchers.len())
            .filter(|&i| {
                let watched = &self.watchers[i].1;
                watched.is_prefix_of(pointer) || pointer.is_prefix_of(watched)
            })
            .collect();
        let before: Vec<Option<Value>> = affected
            .iter()
            .map(|&i| self.get(&self.watchers[i].1).cloned())
            .collect();

        if value.is_null() {
            self.remove(pointer);
        } else {
            write(&mut self.root, pointer, 0, value)?;
        }

        Ok(affected
            .into_iter()
            .zip(before)
            .filter(|(i, before)| self.get(&self.watchers[*i].1) != before.as_ref())
            .map(|(i, _)| self.watchers[i].0)
            .collect())
    }

    /// Watches `pointer`, which need not hold a value yet.
    ///
    /// Several watchers may share a path.
    pub fn watch(&mut self, pointer: &Pointer) -> WatchId {
        let id = WatchId(self.next_watch);
        self.next_watch += 1;
        self.watchers.push((id, pointer.clone()));
        id
    }

    /// Stops watching. An id that is already gone is ignored.
    pub fn unwatch(&mut self, id: WatchId) {
        self.watchers.retain(|(watcher, _)| *watcher != id);
    }

    /// Drops every watcher.
    pub fn dispose(&mut self) {
        self.watchers.clear();
    }

    fn remove(&mut self, pointer: &Pointer) {
        let Some((key, parents)) = pointer.segments().split_last() else {
            self.root = Value::Object(Map::new());
            return;
        };
        let Some(parent) = parents
            .iter()
            .try_fold(&mut self.root, |node, segment| child_mut(node, segment))
        else {
            return;
        };
        match parent {
            Value::Object(map) => {
                map.remove(key);
            }
            Value::Array(elements) => {
                if let Index::Valid(index) = Index::of(key) {
                    if let Some(slot) = elements.get_mut(index) {
                        *slot = Value::Null;
                    }
                }
            }
            _ => {}
        }
    }
}

fn child<'v>(node: &'v Value, segment: &str) -> Option<&'v Value> {
    match node {
        Value::Object(map) => map.get(segment),
        Value::Array(list) => match Index::of(segment) {
            Index::Valid(index) => list.get(index),
            _ => None,
        },
        _ => None,
    }
}

fn child_mut<'v>(node: &'v mut Value, segment: &str) -> Option<&'v mut Value> {
    match node {
        Value::Object(map) => map.get_mut(segment),
        Value::Array(list) => match Index::of(segment) {
            Index::Valid(index) => list.get_mut(index),
            _ => None,
        },
        _ => None,
    }
}

/// Writes `value` below `node`, which sits at the first `depth` segments of
/// `pointer`. A failure leaves `node` untouched: new containers are built
/// aside and attached only once the write beneath them succeeded.
fn write(node: &mut Value, pointer: &Pointer, depth: usize, value: Value) -> Result<(), DataError> {
    let Some(segment) = pointer.segments().get(depth) else {
        *node = value;
        return Ok(());
    };
    if node.is_null() {
        let mut fresh = match Index::of(segment) {
            Index::NotNumeric => Value::Object(Map::new()),
            Index::Valid(_) | Index::TooLarge => Value::Array(Vec::new()),
        };
        write(&mut fresh, pointer, depth, value)?;
        *node = fresh;
        return Ok(());
    }
    match node {
        Value::Object(map) => {
            if let Some(slot) = map.get_mut(segment) {
                return write(slot, pointer, depth + 1, value);
            }
            let mut fresh = Value::Null;
            write(&mut fresh, pointer, depth + 1, value)?;
            map.insert(segment.clone(), fresh);
            Ok(())
        }
        Value::Array(list) => match Index::of(segment) {
            Index::Valid(index) if index < list.len() => {
                write(&mut list[index], pointer, depth + 1, value)
            }
            Index::Valid(index) => {
                let mut fresh = Value::Null;
                write(&mut fresh, pointer, depth + 1, value)?;
                list.resize(index, Value::Null);
                list.push(fresh);
                Ok(())
            }
            Index::NotNumeric => Err(DataError::NonNumericSegment {
                pointer: pointer.clone(),
                segment: segment.clone(),
            }),
            Index::TooLarge => Err(DataError::ListIndexTooLarge {
                pointer: pointer.clone(),
                segment: segment.clone(),
            }),
        },
        _ => Err(DataError::PrimitiveInPath {
            pointer: pointer.clone(),
            at: pointer.truncated(depth),
        }),
    }
}
