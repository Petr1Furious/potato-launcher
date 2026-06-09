use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::{Context, EventEmitter};
use instance::storage::InstanceId;

#[derive(Clone)]
pub struct JavaResolvedEvent(pub InstanceId);

pub enum JavaResolveState {
    Resolving,
    Found(Arc<str>),
    NotFound,
}

#[derive(Default)]
pub struct JavaResolveCache {
    resolving: HashSet<InstanceId>,
    paths: HashMap<InstanceId, Option<Arc<str>>>,
}

impl EventEmitter<JavaResolvedEvent> for JavaResolveCache {}

impl JavaResolveCache {
    pub fn set_resolving(&mut self, instance: InstanceId, cx: &mut Context<Self>) {
        self.resolving.insert(instance.clone());
        cx.emit(JavaResolvedEvent(instance));
        cx.notify();
    }

    pub fn set(&mut self, instance: InstanceId, path: Option<Arc<str>>, cx: &mut Context<Self>) {
        self.resolving.remove(&instance);
        self.paths.insert(instance.clone(), path);
        cx.emit(JavaResolvedEvent(instance));
        cx.notify();
    }

    pub fn state(&self, instance: &InstanceId) -> Option<JavaResolveState> {
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
