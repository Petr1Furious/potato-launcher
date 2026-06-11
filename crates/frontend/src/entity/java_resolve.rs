use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::{Context, EventEmitter};
use instance::storage::InstanceHandle;

#[derive(Clone)]
pub struct JavaResolvedEvent(pub InstanceHandle);

pub enum JavaResolveState {
    Resolving,
    Found(Arc<str>),
    NotFound,
}

#[derive(Default)]
pub struct JavaResolveCache {
    resolving: HashSet<InstanceHandle>,
    paths: HashMap<InstanceHandle, Option<Arc<str>>>,
}

impl EventEmitter<JavaResolvedEvent> for JavaResolveCache {}

impl JavaResolveCache {
    pub fn set_resolving(&mut self, instance: InstanceHandle, cx: &mut Context<Self>) {
        self.resolving.insert(instance.clone());
        cx.emit(JavaResolvedEvent(instance));
        cx.notify();
    }

    pub fn set(
        &mut self,
        instance: InstanceHandle,
        path: Option<Arc<str>>,
        cx: &mut Context<Self>,
    ) {
        self.resolving.remove(&instance);
        self.paths.insert(instance.clone(), path);
        cx.emit(JavaResolvedEvent(instance));
        cx.notify();
    }

    pub fn state(&self, instance: &InstanceHandle) -> Option<JavaResolveState> {
        if self.resolving.contains(instance) {
            return Some(JavaResolveState::Resolving);
        }
        match self.paths.get(instance) {
            None => None,
            Some(None) => Some(JavaResolveState::NotFound),
            Some(Some(p)) => Some(JavaResolveState::Found(p.clone())),
        }
    }
}
