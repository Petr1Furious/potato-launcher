use async_trait::async_trait;
use egui::ComboBox;
use egui::RichText;
use egui::Window;
use image::Luma;
use launcher_auth::AccountData;
use launcher_auth::flow::{AuthMessage, AuthMessageProvider, perform_auth};
use launcher_auth::providers::{
    AuthProviderConfig, AuthProviderType, ElyByAuthProvider, MicrosoftAuthProvider,
    OfflineAuthProvider, TGAuthProvider,
};
use launcher_auth::storage::{AccountDataSource, AuthStorage, StorageEntry};
use log::error;
use qrcode::QrCode;
use shared::paths::get_auth_data_path;
use shared::utils::is_connect_error;
use std::hash::DefaultHasher;
use std::hash::Hash as _;
use std::hash::Hasher as _;
use std::io::Cursor;
use std::sync::Arc;
use tokio::runtime::Runtime;
use tokio::sync::Mutex;
use tokio::sync::mpsc;

use crate::config::build_config;
use crate::config::runtime_config::AuthProfile;
use crate::config::runtime_config::Config;
use crate::lang::{Lang, LangMessage};
use crate::utils::is_valid_minecraft_username;

use super::background_task::{BackgroundTask, BackgroundTaskResult};
use super::colors;

#[derive(Clone, PartialEq)]
enum AuthStatus {
    NotAuthorized,
    Authorized,
    AuthorizeError,
    AuthorizeErrorOffline,
    AuthorizeErrorTimeout,
}

struct AuthResult {
    auth_provider: AuthProviderConfig,
    status: AuthStatus,
    account_data: Option<AccountData>,
}

fn authenticate(
    runtime: &Runtime,
    account_data: Option<AccountData>,
    auth_provider: AuthProviderConfig,
    auth_message_provider: Arc<EguiAuthMessageProvider>,
    ctx: &egui::Context,
) -> BackgroundTask<AuthResult> {
    let ctx = ctx.clone();

    let fut = async move {
        match perform_auth(account_data, auth_provider.clone(), auth_message_provider).await {
            Ok(data) => AuthResult {
                auth_provider: auth_provider.clone(),
                status: AuthStatus::Authorized,
                account_data: Some(data),
            },

            Err(e) => {
                let mut connect_error = false;
                if is_connect_error(&e) {
                    connect_error = true;
                }
                let mut timeout_error = false;
                if let Some(re) = e.downcast_ref::<reqwest::Error>()
                    && (re.is_timeout() || re.status().map(|s| s.as_u16()) == Some(524))
                {
                    timeout_error = true;
                }

                AuthResult {
                    auth_provider,
                    status: if connect_error {
                        AuthStatus::AuthorizeErrorOffline
                    } else if timeout_error {
                        AuthStatus::AuthorizeErrorTimeout
                    } else {
                        error!("Auth error:\n{e:?}");
                        AuthStatus::AuthorizeError
                    },
                    account_data: None,
                }
            }
        }
    };

    BackgroundTask::with_callback(
        fut,
        runtime,
        Box::new(move || {
            ctx.request_repaint();
        }),
    )
}

pub struct AuthState {
    auth_status: AuthStatus,
    auth_task: Option<BackgroundTask<AuthResult>>,
    auth_message_provider: Arc<EguiAuthMessageProvider>,
    auth_storage: AuthStorage,

    show_add_account: bool,

    new_account_type: AuthProviderType,

    ely_by_client_id: String,
    ely_by_client_secret: String,

    telegram_auth_base_url: String,

    offline_nickname: String,

    last_auth_profile: Option<AuthProfile>,
}

impl AuthState {
    pub fn new(ctx: &egui::Context, config: &Config) -> Self {
        let auth_data_path = get_auth_data_path(&config.get_launcher_dir());
        AuthState {
            auth_status: AuthStatus::NotAuthorized,
            auth_task: None,
            auth_message_provider: Arc::new(EguiAuthMessageProvider::new(ctx)),
            auth_storage: AuthStorage::load(auth_data_path),

            show_add_account: false,

            new_account_type: AuthProviderType::Microsoft,

            ely_by_client_id: String::new(),
            ely_by_client_secret: String::new(),

            telegram_auth_base_url: String::new(),

            offline_nickname: String::new(),

            last_auth_profile: None,
        }
    }

    pub fn update(&mut self, runtime: &Runtime, config: &mut Config) -> bool {
        if let Some(task) = self.auth_task.as_ref()
            && task.has_result()
        {
            runtime.block_on(self.auth_message_provider.clear());
            let task = self.auth_task.take().unwrap();
            let result = task.take_result();
            match result {
                BackgroundTaskResult::Finished(result) => {
                    if result.status == AuthStatus::Authorized
                        && let Some(auth_data) = result.account_data
                    {
                        config.set_selected_auth_profile(AuthProfile {
                            auth_backend_id: result.auth_provider.get_id(),
                            username: auth_data.user_info.username.clone(),
                        });

                        self.auth_storage.insert(&result.auth_provider, auth_data);
                        self.show_add_account = false;
                    }

                    self.auth_status = result.status;
                }

                BackgroundTaskResult::Cancelled => {
                    self.auth_status = AuthStatus::NotAuthorized;
                }
            }

            return true;
        }

        false
    }

    fn render_auth_window(&mut self, config: &mut Config, runtime: &Runtime, ui: &mut egui::Ui) {
        if let Some(message) = runtime.block_on(self.auth_message_provider.get_lang_message()) {
            let lang = config.lang;
            let ctx = ui.ctx();

            egui::Window::new(LangMessage::Authorization.to_string(lang)).show(ctx, |ui| {
                ui.label(message.to_string(lang));
                let url = match message {
                    LangMessage::AuthMessage { url } => Some(url),
                    LangMessage::DeviceAuthMessage { url, .. } => Some(url),
                    _ => None,
                }
                .unwrap();

                ui.hyperlink(&url);
                let code = QrCode::new(url).unwrap();
                let image = code.render::<Luma<u8>>().build();

                let mut png_bytes: Vec<u8> = Vec::new();
                let mut cursor = Cursor::new(&mut png_bytes);
                image::DynamicImage::ImageLuma8(image)
                    .write_to(&mut cursor, image::ImageFormat::Png)
                    .unwrap();

                let uri = "bytes://auth_qr.png";
                ctx.include_bytes(uri, png_bytes.clone());
                ui.add(egui::Image::from_bytes(uri.to_string(), png_bytes));

                if ui.button(LangMessage::Cancel.to_string(lang)).clicked() {
                    self.auth_status = AuthStatus::NotAuthorized;
                    self.auth_task = None;
                    self.auth_message_provider = Arc::new(EguiAuthMessageProvider::new(ctx));
                    self.on_instance_changed(config, runtime, ctx);
                }
            });
        }

        if runtime.block_on(self.auth_message_provider.need_offline_nickname()) {
            let lang = config.lang;
            let ctx = ui.ctx();

            let mut open = true;
            egui::Window::new(LangMessage::Authorization.to_string(lang))
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(LangMessage::EnterNickname.to_string(lang));
                            ui.text_edit_singleline(&mut self.offline_nickname);
                        });

                        if ui
                            .add_enabled(
                                is_valid_minecraft_username(&self.offline_nickname),
                                egui::Button::new(LangMessage::AddAccount.to_string(lang)),
                            )
                            .clicked()
                        {
                            runtime.block_on(
                                self.auth_message_provider
                                    .set_offline_nickname(self.offline_nickname.clone()),
                            );
                        }
                    });
                });
            if !open {
                self.auth_status = AuthStatus::NotAuthorized;
                self.auth_task = None;
                self.auth_message_provider = Arc::new(EguiAuthMessageProvider::new(ctx));
                self.on_instance_changed(config, runtime, ctx);
            }
        }
    }

    fn get_type_display_name(lang: Lang, new_account_type: AuthProviderType) -> String {
        match new_account_type {
            AuthProviderType::Microsoft => "Microsoft".to_string(),
            AuthProviderType::ElyBy => "Ely.by".to_string(),
            AuthProviderType::Telegram => "Telegram".to_string(),
            AuthProviderType::Offline => LangMessage::Offline.to_string(lang),
        }
    }

    pub fn render_new_account_window(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        runtime: &Runtime,
        lang: Lang,
    ) {
        let mut show_add_account = self.show_add_account;
        Window::new(LangMessage::AddAccount.to_string(lang))
            .open(&mut show_add_account)
            .show(ui.ctx(), |ui| {
                ui.label(LangMessage::SelectAccount.to_string(lang));
                ComboBox::from_id_salt("new_account_type")
                    .selected_text(Self::get_type_display_name(lang, self.new_account_type))
                    .show_ui(ui, |ui| {
                        for account_type in AuthProviderType::iter() {
                            ui.selectable_value(
                                &mut self.new_account_type,
                                account_type,
                                Self::get_type_display_name(lang, account_type),
                            );
                        }
                    });

                match self.new_account_type {
                    AuthProviderType::Microsoft => {}
                    AuthProviderType::ElyBy => {
                        ui.horizontal(|ui| {
                            ui.label("Client ID:");
                            ui.text_edit_singleline(&mut self.ely_by_client_id);
                        });
                        ui.horizontal(|ui| {
                            ui.label("Client Secret:");
                            ui.text_edit_singleline(&mut self.ely_by_client_secret);
                        });
                    }
                    AuthProviderType::Telegram => {
                        ui.horizontal(|ui| {
                            ui.label("Auth Base URL:");
                            ui.text_edit_singleline(&mut self.telegram_auth_base_url);
                        });
                    }
                    AuthProviderType::Offline => {}
                }

                if ui
                    .button(LangMessage::AddAndAuthenticate.to_string(lang))
                    .clicked()
                {
                    let new_auth_provider = match self.new_account_type {
                        AuthProviderType::Microsoft => {
                            AuthProviderConfig::Microsoft(MicrosoftAuthProvider {})
                        }
                        AuthProviderType::ElyBy => AuthProviderConfig::ElyBy(ElyByAuthProvider {
                            client_id: self.ely_by_client_id.clone(),
                            client_secret: self.ely_by_client_secret.clone(),
                            launcher_name: build_config::get_launcher_name(),
                        }),
                        AuthProviderType::Telegram => {
                            AuthProviderConfig::Telegram(TGAuthProvider {
                                auth_base_url: self.telegram_auth_base_url.clone(),
                            })
                        }
                        AuthProviderType::Offline => {
                            AuthProviderConfig::Offline(OfflineAuthProvider {})
                        }
                    };

                    self.auth_status = AuthStatus::NotAuthorized;
                    self.auth_message_provider = Arc::new(EguiAuthMessageProvider::new(ctx));
                    self.auth_task = Some(authenticate(
                        runtime,
                        None,
                        new_auth_provider,
                        self.auth_message_provider.clone(),
                        ctx,
                    ));

                    self.show_add_account = false;
                }
            });
        self.show_add_account = show_add_account;
    }

    fn get_selected_storage_entry(&self, config: &Config) -> Option<StorageEntry> {
        let auth_profile = config.get_selected_auth_profile()?;
        self.auth_storage
            .get_by_id(&auth_profile.auth_backend_id, &auth_profile.username)
    }

    fn on_instance_changed(&mut self, config: &mut Config, runtime: &Runtime, ctx: &egui::Context) {
        self.auth_status = AuthStatus::NotAuthorized;

        let mut auth_profile = config.get_selected_auth_profile().cloned();

        if let Some(profile) = &auth_profile
            && self
                .auth_storage
                .get_by_id(&profile.auth_backend_id, &profile.username)
                .is_none()
        {
            config.clear_selected_auth_profile();
            auth_profile = None;
        }

        let storage_entry = self.get_selected_storage_entry(config);
        if let Some(storage_entry) = &storage_entry {
            if storage_entry.source == AccountDataSource::Persistent && self.auth_task.is_none() {
                self.auth_message_provider = Arc::new(EguiAuthMessageProvider::new(ctx));
                self.auth_task = Some(authenticate(
                    runtime,
                    Some(storage_entry.auth_data.clone()),
                    AuthProviderConfig::from_id(&auth_profile.as_ref().unwrap().auth_backend_id),
                    self.auth_message_provider.clone(),
                    ctx,
                ));
            }
            if storage_entry.source == AccountDataSource::Runtime {
                self.auth_status = AuthStatus::Authorized;
            }
        }
    }

    fn render_buttons(
        &mut self,
        ui: &mut egui::Ui,
        config: &mut Config,
        runtime: &Runtime,
        instance_auth_provider: Option<AuthProviderConfig>,
    ) {
        let mut auth_profile = config.get_selected_auth_profile().cloned();

        if ui
            .add_enabled(auth_profile.is_some(), egui::Button::new("-"))
            .clicked()
            && let Some(auth_profile) = auth_profile.take()
        {
            self.auth_storage
                .delete_by_id(&auth_profile.auth_backend_id, &auth_profile.username);
            config.clear_selected_auth_profile();
        }

        if ui.button("+").clicked() {
            if let Some(instance_auth_provider) = instance_auth_provider {
                let ctx = ui.ctx();

                self.auth_status = AuthStatus::NotAuthorized;
                self.auth_message_provider = Arc::new(EguiAuthMessageProvider::new(ctx));
                self.auth_task = Some(authenticate(
                    runtime,
                    None,
                    instance_auth_provider,
                    self.auth_message_provider.clone(),
                    ctx,
                ));
            } else {
                self.show_add_account = true;

                self.new_account_type = AuthProviderType::Microsoft;

                self.ely_by_client_id = String::new();
                self.ely_by_client_secret = String::new();

                self.telegram_auth_base_url = String::new();

                self.offline_nickname = String::new();
            }
        }
    }

    fn get_account_display_name(lang: Lang, id: &String, username: &String) -> String {
        let provider_config = AuthProviderConfig::from_id(id);
        let provider_name = AuthState::get_type_display_name(lang, provider_config.get_type());

        let mut hasher = DefaultHasher::new();
        id.hash(&mut hasher);
        let hash = hasher.finish();

        let hex = format!("{hash:X}");
        format!("{} ({} #{})", username, provider_name, &hex[0..4])
    }

    fn get_combobox_text(
        nickname: &str,
        status: &AuthStatus,
        lang: Lang,
        dark_mode: bool,
    ) -> RichText {
        match status {
            AuthStatus::Authorized => RichText::new(nickname).color(colors::ok(dark_mode)),
            AuthStatus::NotAuthorized => RichText::new(format!(
                "{} ({})",
                nickname,
                LangMessage::Authorizing.to_string(lang)
            ))
            .color(colors::in_progress(dark_mode)),
            AuthStatus::AuthorizeError => RichText::new(format!(
                "{} ({})",
                nickname,
                LangMessage::UnknownAuthError.to_string(lang)
            ))
            .color(colors::error(dark_mode)),
            AuthStatus::AuthorizeErrorOffline => RichText::new(format!(
                "{} ({})",
                nickname,
                LangMessage::Offline.to_string(lang)
            ))
            .color(colors::offline(dark_mode)),
            AuthStatus::AuthorizeErrorTimeout => RichText::new(format!(
                "{} ({})",
                nickname,
                LangMessage::AuthTimeout.to_string(lang)
            ))
            .color(colors::timeout(dark_mode)),
        }
    }

    pub fn render_ui(
        &mut self,
        ui: &mut egui::Ui,
        config: &mut Config,
        runtime: &Runtime,
        ctx: &egui::Context,
        instance_auth_provider: Option<AuthProviderConfig>,
    ) {
        let lang = config.lang;

        let auth_profile = config.get_selected_auth_profile().cloned();
        if auth_profile.is_none()
            && let Some(instance_auth_backend) = instance_auth_provider.clone()
        {
            let entries = self
                .auth_storage
                .get_id_nicknames(&instance_auth_backend.get_id());
            if let Some(nickname) = entries.first() {
                let auth_profile_value = AuthProfile {
                    auth_backend_id: instance_auth_backend.get_id(),
                    username: nickname.clone(),
                };
                config.set_selected_auth_profile(auth_profile_value.clone());
            }
        }

        if self.last_auth_profile != auth_profile {
            self.on_instance_changed(config, runtime, ctx);
            self.last_auth_profile = auth_profile.clone();
        }

        let dark_mode = ui.style().visuals.dark_mode;

        let auth_profile = config.get_selected_auth_profile().cloned();
        if let Some(instance_auth_provider) = instance_auth_provider {
            let mut entries = self
                .auth_storage
                .get_id_nicknames(&instance_auth_provider.get_id());

            if !entries.is_empty() {
                self.render_buttons(ui, config, runtime, Some(instance_auth_provider.clone()));

                let mut selected_username = auth_profile.as_ref().map(|x| x.username.to_string());
                ComboBox::from_id_salt("select_account")
                    .selected_text(match &selected_username {
                        Some(username) => {
                            Self::get_combobox_text(username, &self.auth_status, lang, dark_mode)
                        }
                        None => RichText::new(LangMessage::SelectAccount.to_string(lang))
                            .color(colors::action(dark_mode)),
                    })
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        if config.selected_instance_name.is_none() {
                            ui.disable();
                            return;
                        }
                        entries.sort();
                        for username in entries {
                            ui.selectable_value(
                                &mut selected_username,
                                Some(username.clone()),
                                username,
                            );
                        }
                    });
                if let Some(selected_username) = selected_username
                    && auth_profile.as_ref().map(|x| &x.username) != Some(&selected_username)
                {
                    let auth_profile_value = AuthProfile {
                        auth_backend_id: instance_auth_provider.get_id(),
                        username: selected_username.clone(),
                    };
                    config.set_selected_auth_profile(auth_profile_value.clone());
                }
            } else {
                ui.add_enabled_ui(self.auth_task.is_none(), |ui| {
                    let button_text =
                        if !matches!(instance_auth_provider, AuthProviderConfig::Offline(_)) {
                            egui::RichText::new(
                                LangMessage::AuthorizeUsing(AuthState::get_type_display_name(
                                    lang,
                                    instance_auth_provider.get_type(),
                                ))
                                .to_string(lang),
                            )
                        } else {
                            egui::RichText::new(LangMessage::AddOfflineAccount.to_string(lang))
                        }
                        .size(20.0);

                    if ui
                        .add_sized([ui.available_width(), 50.0], egui::Button::new(button_text))
                        .clicked()
                    {
                        let storage_entry = self.get_selected_storage_entry(config);

                        self.auth_status = AuthStatus::NotAuthorized;
                        self.auth_message_provider = Arc::new(EguiAuthMessageProvider::new(ctx));
                        self.auth_task = Some(authenticate(
                            runtime,
                            storage_entry.as_ref().map(|x| x.auth_data.clone()),
                            instance_auth_provider,
                            self.auth_message_provider.clone(),
                            ctx,
                        ));
                    }
                });
            }
        } else {
            self.render_buttons(ui, config, runtime, instance_auth_provider);

            let mut all_entries = self.auth_storage.get_all_entries();

            let mut selected_account = auth_profile
                .as_ref()
                .map(|x| (x.auth_backend_id.clone(), x.username.clone()));
            ComboBox::from_id_salt("select_account")
                .selected_text(match &selected_account {
                    Some((_, username)) => {
                        Self::get_combobox_text(username, &self.auth_status, lang, dark_mode)
                    }
                    None => RichText::new(LangMessage::SelectAccount.to_string(lang))
                        .color(colors::action(dark_mode)),
                })
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    if config.selected_instance_name.is_none() {
                        ui.disable();
                        return;
                    }
                    all_entries.sort();
                    for (id, username) in all_entries {
                        ui.selectable_value(
                            &mut selected_account,
                            Some((id.clone(), username.clone())),
                            Self::get_account_display_name(lang, &id, &username),
                        );
                    }
                });
            if let Some(selected_account) = selected_account
                && auth_profile
                    .as_ref()
                    .map(|x| (&x.auth_backend_id, &x.username))
                    != Some((&selected_account.0, &selected_account.1))
            {
                let auth_profile_value = AuthProfile {
                    auth_backend_id: selected_account.0,
                    username: selected_account.1,
                };
                config.set_selected_auth_profile(auth_profile_value.clone());
            }
        }

        self.render_new_account_window(ui, ctx, runtime, lang);
        self.render_auth_window(config, runtime, ui);
    }

    pub fn get_auth_data(&self, config: &Config) -> Option<AccountData> {
        let profile = config.get_selected_auth_profile()?;
        if let Some(storage_entry) = self
            .auth_storage
            .get_by_id(&profile.auth_backend_id, &profile.username)
        {
            match storage_entry.source {
                AccountDataSource::Runtime => {
                    return Some(storage_entry.auth_data);
                }
                AccountDataSource::Persistent => {
                    if self.auth_status == AuthStatus::AuthorizeErrorOffline {
                        return Some(storage_entry.auth_data); // allow playing with an existing account when offline
                    }
                    return None;
                }
            }
        }
        None
    }

    pub fn offline(&self) -> bool {
        matches!(
            self.auth_status,
            AuthStatus::AuthorizeErrorOffline
                | AuthStatus::AuthorizeErrorTimeout
                | AuthStatus::AuthorizeError
        )
    }

    pub fn reset(&mut self, config: &mut Config, runtime: &Runtime, ctx: &egui::Context) {
        self.on_instance_changed(config, runtime, ctx);
    }
}

struct AuthMessageState {
    auth_message: Option<AuthMessage>,
    need_offline_nickname: u32,
}

struct EguiAuthMessageProvider {
    state: Arc<Mutex<AuthMessageState>>,
    offline_nickname_sender: mpsc::UnboundedSender<String>,
    offline_nickname_receiver: Arc<Mutex<mpsc::UnboundedReceiver<String>>>,
    ctx: egui::Context,
}

impl EguiAuthMessageProvider {
    fn new(ctx: &egui::Context) -> Self {
        let (sender, receiver) = mpsc::unbounded_channel();
        Self {
            state: Arc::new(Mutex::new(AuthMessageState {
                auth_message: None,
                need_offline_nickname: 0,
            })),
            offline_nickname_sender: sender,
            offline_nickname_receiver: Arc::new(Mutex::new(receiver)),
            ctx: ctx.clone(),
        }
    }

    async fn get_lang_message(&self) -> Option<LangMessage> {
        self.get_message()
            .await
            .map(|auth_message| match auth_message {
                AuthMessage::Link { url } => LangMessage::AuthMessage { url },
                AuthMessage::LinkCode { url, code } => LangMessage::DeviceAuthMessage { url, code },
            })
    }
}

#[async_trait]
impl AuthMessageProvider for EguiAuthMessageProvider {
    async fn set_message(&self, message: AuthMessage) {
        let mut state = self.state.lock().await;
        state.auth_message = Some(message);
        self.ctx.request_repaint();
    }

    async fn get_message(&self) -> Option<AuthMessage> {
        let state = self.state.lock().await;
        state.auth_message.clone()
    }

    async fn clear(&self) {
        let mut state = self.state.lock().await;
        state.auth_message = None;
        self.ctx.request_repaint();
    }

    async fn request_offline_nickname(&self) -> String {
        {
            let mut state = self.state.lock().await;
            state.need_offline_nickname += 1;
        }

        self.offline_nickname_receiver
            .lock()
            .await
            .recv()
            .await
            .unwrap()
    }

    async fn need_offline_nickname(&self) -> bool {
        let state = self.state.lock().await;
        state.need_offline_nickname > 0
    }

    async fn set_offline_nickname(&self, nickname: String) {
        let mut state = self.state.lock().await;
        state.need_offline_nickname -= 1;
        self.offline_nickname_sender.send(nickname).unwrap();
    }
}
