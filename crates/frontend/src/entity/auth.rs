use std::sync::Arc;

use gpui::{Context, EventEmitter, RenderImage};
use launcher_auth::flow::AuthMessage;
use launcher_bridge::AuthPromptContext;

#[derive(Clone, Default)]
pub struct AuthSession {
    pub context: Option<AuthPromptContext>,
    pub message: Option<AuthMessage>,
    pub qr_image: Option<Arc<RenderImage>>,
}

#[derive(Clone)]
pub struct AuthSessionUpdatedEvent;

impl EventEmitter<AuthSessionUpdatedEvent> for AuthSession {}

impl AuthSession {
    pub fn set_prompt(
        &mut self,
        context: AuthPromptContext,
        message: AuthMessage,
        cx: &mut Context<Self>,
    ) {
        let url = match &message {
            AuthMessage::Link { url } | AuthMessage::LinkCode { url, .. } => url.clone(),
        };
        self.context = Some(context);
        self.message = Some(message);
        self.qr_image = crate::auth_qr::qr_image_for_url(&url);
        cx.emit(AuthSessionUpdatedEvent);
        cx.notify();
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.context = None;
        self.message = None;
        self.qr_image = None;
        cx.emit(AuthSessionUpdatedEvent);
        cx.notify();
    }

    pub fn is_active(&self) -> bool {
        self.message.is_some()
    }
}
