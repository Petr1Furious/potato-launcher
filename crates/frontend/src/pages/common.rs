use std::{collections::BTreeMap, process::Command};

use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::{App, Hsla, IntoElement, ParentElement, Styled, div, prelude::*};
use launcher_auth::providers::AuthProviderConfig;
use launcher_bridge::{
    AccountView, BackendFetchState, BackendStatus, InstanceLiveStatus, InstanceOrigin, InstanceView,
};
use launcher_i18n as t;
use url::Url;

pub fn page_header(title: impl Into<gpui_kit::SharedString>, cx: &App) -> gpui_kit::Div {
    h_flex().w_full().items_center().gap_3().child(
        div()
            .text_2xl()
            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
            .text_color(cx.theme().foreground)
            .child(title.into()),
    )
}

pub fn group(title: impl Into<gpui_kit::SharedString>, cx: &App) -> gpui_kit::Div {
    v_flex().w_full().min_w_0().gap_2().child(
        div()
            .text_xs()
            .font_weight(gpui_kit::FontWeight::MEDIUM)
            .text_color(cx.theme().muted_foreground)
            .child(title.into()),
    )
}

pub fn group_box(cx: &App) -> gpui_kit::Div {
    v_flex()
        .w_full()
        .min_w_0()
        .rounded(cx.theme().radius_lg)
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().group_box)
}

pub fn setting_row(
    title: impl Into<gpui_kit::SharedString>,
    description: Option<gpui_kit::SharedString>,
    control: impl IntoElement,
    cx: &App,
) -> gpui_kit::Div {
    h_flex()
        .w_full()
        .min_w_0()
        .justify_between()
        .items_center()
        .gap_4()
        .px_4()
        .py_3()
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(div().text_sm().child(title.into()))
                .when_some(description, |this, description| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(description),
                    )
                }),
        )
        .child(div().flex_none().child(control))
}

pub fn row_divider(cx: &App) -> gpui_kit::Div {
    div().w_full().h_px().bg(cx.theme().border)
}

pub fn muted(text: impl Into<gpui_kit::SharedString>, cx: &App) -> gpui_kit::Div {
    div()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

pub fn error_alert(message: String, cx: &App) -> gpui_kit::Div {
    div()
        .w_full()
        .p_3()
        .rounded(cx.theme().radius)
        .border_1()
        .border_color(cx.theme().danger.opacity(0.5))
        .bg(cx.theme().danger.opacity(0.1))
        .text_sm()
        .child(message)
}

pub fn status_dot(color: Hsla) -> gpui_kit::Div {
    div().flex_none().size_2().rounded_full().bg(color)
}

pub fn fetch_state_color(fetch_state: &BackendFetchState, cx: &App) -> Hsla {
    match fetch_state {
        BackendFetchState::Offline | BackendFetchState::Error(_) => cx.theme().danger,
        BackendFetchState::Fetching => cx.theme().warning,
        BackendFetchState::Fetched { .. } => cx.theme().success,
        BackendFetchState::NotFetched => cx.theme().muted_foreground,
    }
}

pub fn fetch_state_label(fetch_state: &BackendFetchState) -> String {
    match fetch_state {
        BackendFetchState::NotFetched => t::backends::not_fetched().to_string(),
        BackendFetchState::Fetching => t::common::refreshing().to_string(),
        BackendFetchState::Fetched { instance_count } => {
            t::backends::published_count(*instance_count)
        }
        BackendFetchState::Offline => t::backends::offline().to_string(),
        BackendFetchState::Error(error) => error.to_string(),
    }
}

pub fn game_label(instance: &InstanceView) -> Option<String> {
    match (&instance.minecraft_version, &instance.loader) {
        (Some(minecraft), Some(loader)) => Some(format!("{minecraft} · {loader}")),
        (Some(minecraft), None) => Some(minecraft.to_string()),
        (None, Some(loader)) => Some(loader.to_string()),
        (None, None) => None,
    }
}

pub fn status_label(instance: &InstanceView) -> String {
    let label = match &instance.status {
        InstanceLiveStatus::NotInstalled => t::instances::status_available().to_string(),
        InstanceLiveStatus::Installed => t::instances::status_installed().to_string(),
        InstanceLiveStatus::Outdated => t::instances::status_outdated().to_string(),
        InstanceLiveStatus::OutdatedOffline => t::instances::status_outdated_offline().to_string(),
        InstanceLiveStatus::Installing { .. } => t::progress::installing().to_string(),
        InstanceLiveStatus::InstallFailed(_) => t::instances::status_failed().to_string(),
        InstanceLiveStatus::LaunchPreparing { .. } => t::instances::launching().to_string(),
        InstanceLiveStatus::Running => t::instances::status_running().to_string(),
        InstanceLiveStatus::LaunchFailed(_) => t::instances::status_launch_failed().to_string(),
        InstanceLiveStatus::OrphanedFromBackend => t::instances::status_orphaned().to_string(),
        InstanceLiveStatus::NeedsLauncherUpdate => {
            t::instances::status_needs_launcher_update().to_string()
        }
    };
    if instance.launch_blocked_reason.is_some()
        && matches!(
            instance.status,
            InstanceLiveStatus::Installed
                | InstanceLiveStatus::Outdated
                | InstanceLiveStatus::OutdatedOffline
                | InstanceLiveStatus::OrphanedFromBackend
                | InstanceLiveStatus::NeedsLauncherUpdate
                | InstanceLiveStatus::LaunchFailed(_)
        )
    {
        t::instances::status_no_account(label)
    } else {
        label
    }
}

pub fn status_error(status: &InstanceLiveStatus) -> Option<String> {
    match status {
        InstanceLiveStatus::InstallFailed(error) | InstanceLiveStatus::LaunchFailed(error) => {
            Some(error.to_string())
        }
        _ => None,
    }
}

pub enum StatusTone {
    Neutral,
    Good,
    Warning,
    Danger,
}

/// `None` for the resting state, which needs no badge.
pub fn status_tone(instance: &InstanceView) -> Option<StatusTone> {
    if instance.launch_blocked_reason.is_some() {
        return Some(StatusTone::Warning);
    }
    match &instance.status {
        InstanceLiveStatus::Installed => None,
        InstanceLiveStatus::Running => Some(StatusTone::Good),
        InstanceLiveStatus::NotInstalled
        | InstanceLiveStatus::Installing { .. }
        | InstanceLiveStatus::LaunchPreparing { .. } => Some(StatusTone::Neutral),
        InstanceLiveStatus::Outdated
        | InstanceLiveStatus::OutdatedOffline
        | InstanceLiveStatus::NeedsLauncherUpdate
        | InstanceLiveStatus::OrphanedFromBackend => Some(StatusTone::Warning),
        InstanceLiveStatus::InstallFailed(_) | InstanceLiveStatus::LaunchFailed(_) => {
            Some(StatusTone::Danger)
        }
    }
}

pub fn status_badge(instance: &InstanceView, cx: &App) -> Option<gpui_kit::Div> {
    let tone = status_tone(instance)?;
    let color = match tone {
        StatusTone::Neutral => cx.theme().muted_foreground,
        StatusTone::Good => cx.theme().success,
        StatusTone::Warning => cx.theme().warning,
        StatusTone::Danger => cx.theme().danger,
    };
    Some(
        h_flex()
            .flex_none()
            .items_center()
            .gap_1p5()
            .px_2()
            .py_0p5()
            .rounded_full()
            .bg(color.opacity(0.12))
            .text_xs()
            .text_color(color)
            .child(status_dot(color))
            .child(status_label(instance)),
    )
}

pub fn can_launch(instance: &InstanceView) -> bool {
    matches!(
        instance.status,
        InstanceLiveStatus::Installed
            | InstanceLiveStatus::OutdatedOffline
            | InstanceLiveStatus::OrphanedFromBackend
            | InstanceLiveStatus::LaunchFailed(_)
    ) || (instance.locally_installed
        && matches!(instance.status, InstanceLiveStatus::NeedsLauncherUpdate))
}

pub fn instance_busy(status: &InstanceLiveStatus) -> bool {
    matches!(
        status,
        InstanceLiveStatus::Running
            | InstanceLiveStatus::LaunchPreparing { .. }
            | InstanceLiveStatus::Installing { .. }
    )
}

pub fn is_local(instance: &InstanceView) -> bool {
    matches!(instance.origin, InstanceOrigin::Local)
}

pub fn provider_label(provider: &AuthProviderConfig) -> String {
    match provider {
        AuthProviderConfig::Microsoft(_) => t::providers::microsoft().to_string(),
        AuthProviderConfig::Telegram(provider) => {
            t::providers::telegram_with_url(provider.auth_base_url.clone())
        }
        AuthProviderConfig::ElyBy(_) => t::providers::elyby().to_string(),
        AuthProviderConfig::Offline(_) => t::providers::offline().to_string(),
    }
}

pub fn provider_type_label(provider: &AuthProviderConfig) -> String {
    match provider {
        AuthProviderConfig::Microsoft(_) => t::providers::microsoft().to_string(),
        AuthProviderConfig::Telegram(_) => t::providers::telegram().to_string(),
        AuthProviderConfig::ElyBy(_) => t::providers::elyby().to_string(),
        AuthProviderConfig::Offline(_) => t::providers::offline().to_string(),
    }
}

/// Accounts the instance can launch with: those of its required provider, or all.
pub fn compatible_accounts(instance: &InstanceView, accounts: &[AccountView]) -> Vec<AccountView> {
    accounts
        .iter()
        .filter(|account| {
            instance
                .auth_provider
                .as_ref()
                .is_none_or(|provider| &account.provider == provider)
        })
        .cloned()
        .collect()
}

pub fn open_path(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("open");
        command.arg(path);
        command
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", ""]).arg(path);
        command
    };
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(path);
        command
    };

    command.spawn().map(|_| ())
}

pub fn backend_display_names(backends: &[BackendStatus]) -> BTreeMap<String, String> {
    let mut host_counts = BTreeMap::<String, usize>::new();
    for backend in backends {
        if let Some(host) = backend.url.host_str() {
            *host_counts.entry(host.to_string()).or_default() += 1;
        }
    }

    backends
        .iter()
        .map(|backend| {
            let duplicate_host = backend
                .url
                .host_str()
                .and_then(|host| host_counts.get(host))
                .is_some_and(|count| *count > 1);
            (
                backend.url.as_str().to_string(),
                backend_display_name(&backend.url, duplicate_host),
            )
        })
        .collect()
}

pub fn backend_display_name(url: &Url, include_path: bool) -> String {
    let Some(host) = url.host_str() else {
        return url.as_str().to_string();
    };
    if !include_path {
        return host.to_string();
    }

    let mut path = url.path().trim_end_matches('/').to_string();
    if path.is_empty() {
        path = "/".to_string();
    }
    format!("{host}{path}")
}

pub fn avatar(name: &str) -> gpui_kit::component::avatar::Avatar {
    let trimmed = name.trim_matches(|c: char| !c.is_alphanumeric());
    gpui_kit::component::avatar::Avatar::new().name(if trimmed.is_empty() {
        name.to_string()
    } else {
        trimmed.to_string()
    })
}

/// A one-of-many choice; the selected option stands out as a raised pill.
pub fn segmented(
    id: impl Into<gpui_kit::ElementId>,
    labels: impl IntoIterator<Item = String>,
    selected: usize,
    on_select: impl Fn(usize, &mut gpui_kit::Window, &mut App) + 'static,
) -> gpui_kit::component::tab::TabBar {
    use gpui_kit::component::Sizable as _;
    gpui_kit::component::tab::TabBar::new(id)
        .segmented()
        .small()
        .selected_index(selected)
        .children(
            labels
                .into_iter()
                .map(|label| gpui_kit::component::tab::Tab::new().label(label)),
        )
        .on_click(move |index, window, cx| on_select(*index, window, cx))
}
