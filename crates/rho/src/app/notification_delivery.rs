//! Serializes notification publication with the parent's collection handoff.
//!
//! Lock order: delivery gate, then source registry/binding, then record/receipt.
//! Never acquire the gate while holding a source lock, or hold it across await.
//! The gate is process-wide because CLI tools can share managers across session
//! rebuilds. It protects only short publication/snapshot operations, not work.
use std::sync::{Mutex, MutexGuard};

static DELIVERY: Mutex<()> = Mutex::new(());

pub(crate) fn lock() -> MutexGuard<'static, ()> {
    DELIVERY.lock().expect("notification delivery gate")
}

#[cfg(test)]
#[path = "notification_delivery_tests.rs"]
mod tests;
