//! Serializes notification publication with the parent's collection snapshot.
//! Delivery acknowledgements happen outside this gate. Explicit status reads
//! can also report notices already reserved by an in-flight boundary batch.
//!
//! Lock order: delivery gate, then source registry/binding, then record/receipt.
//! Never acquire the gate while holding a source lock, or hold it across await.
//! The gate is process-wide because CLI tools can share managers across session
//! rebuilds. It protects only short publication/snapshot operations, not work.
use std::sync::{Mutex, MutexGuard};

static DELIVERY: Mutex<()> = Mutex::new(());

pub(crate) fn lock() -> MutexGuard<'static, ()> {
    DELIVERY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
#[path = "notification_delivery_tests.rs"]
mod tests;
