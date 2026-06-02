use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::{Context, EventEmitter};
use uuid::Uuid;

#[derive(Clone)]
pub struct JavaResolvedEvent(pub Uuid);

pub enum JavaResolveState {
    Resolving,
    Found(Arc<str>),
    NotFound,
}

#[derive(Default)]
pub struct JavaResolveCache {
    resolving: HashSet<Uuid>,
    paths: HashMap<Uuid, Option<Arc<str>>>,
}

impl EventEmitter<JavaResolvedEvent> for JavaResolveCache {}

impl JavaResolveCache {
    pub fn set_resolving(&mut self, instance: Uuid, cx: &mut Context<Self>) {
        self.resolving.insert(instance);
        cx.emit(JavaResolvedEvent(instance));
        cx.notify();
    }

    pub fn set(&mut self, instance: Uuid, path: Option<Arc<str>>, cx: &mut Context<Self>) {
        self.resolving.remove(&instance);
        self.paths.insert(instance, path);
        cx.emit(JavaResolvedEvent(instance));
        cx.notify();
    }

    pub fn state(&self, instance: Uuid) -> Option<JavaResolveState> {
        if self.resolving.contains(&instance) {
            return Some(JavaResolveState::Resolving);
        }
        match self.paths.get(&instance) {
            None => None,
            Some(None) => Some(JavaResolveState::NotFound),
            Some(Some(p)) => Some(JavaResolveState::Found(p.clone())),
        }
    }
}
