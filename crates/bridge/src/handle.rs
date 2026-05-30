use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::message::{MessageToBackend, MessageToFrontend};

#[derive(Clone, Debug)]
pub struct BackendSender {
    sender: UnboundedSender<MessageToBackend>,
}

#[derive(Debug)]
pub struct BackendReceiver {
    receiver: UnboundedReceiver<MessageToBackend>,
}

#[derive(Clone, Debug)]
pub struct FrontendSender {
    sender: UnboundedSender<MessageToFrontend>,
}

#[derive(Debug)]
pub struct FrontendReceiver {
    receiver: UnboundedReceiver<MessageToFrontend>,
}

pub fn channel() -> (
    BackendSender,
    BackendReceiver,
    FrontendSender,
    FrontendReceiver,
) {
    let (backend_sender, backend_receiver) = unbounded_channel();
    let (frontend_sender, frontend_receiver) = unbounded_channel();
    (
        BackendSender {
            sender: backend_sender,
        },
        BackendReceiver {
            receiver: backend_receiver,
        },
        FrontendSender {
            sender: frontend_sender,
        },
        FrontendReceiver {
            receiver: frontend_receiver,
        },
    )
}

impl BackendSender {
    pub fn send(&self, message: MessageToBackend) {
        let _ = self.sender.send(message);
    }

    pub fn is_closed(&self) -> bool {
        self.sender.is_closed()
    }
}

impl BackendReceiver {
    pub async fn recv(&mut self) -> Option<MessageToBackend> {
        self.receiver.recv().await
    }

    pub fn try_recv(&mut self) -> Option<MessageToBackend> {
        self.receiver.try_recv().ok()
    }
}

impl FrontendSender {
    pub fn send(&self, message: MessageToFrontend) {
        let _ = self.sender.send(message);
    }

    pub fn is_closed(&self) -> bool {
        self.sender.is_closed()
    }
}

impl FrontendReceiver {
    pub async fn recv(&mut self) -> Option<MessageToFrontend> {
        self.receiver.recv().await
    }

    pub fn try_recv(&mut self) -> Option<MessageToFrontend> {
        self.receiver.try_recv().ok()
    }
}
