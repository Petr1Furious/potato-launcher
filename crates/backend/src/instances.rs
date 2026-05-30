use std::{
    collections::{HashMap, HashSet, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
    sync::Arc,
};

use instance::{manifest::InstanceManifest, storage::LocalInstance};
use launcher_auth::{providers::AuthProviderConfig, storage::AccountKey};
use launcher_bridge::{
    AccountView, InstanceLiveStatus, InstanceOrigin, InstanceView, ProgressStage,
};
use url::Url;
use uuid::Uuid;

use crate::catalog::BackendCatalogState;

#[derive(Clone, Debug)]
pub struct InstallProgressView {
    pub stage: ProgressStage,
    pub current: u64,
    pub total: u64,
    pub message: Arc<str>,
    pub show_bar: bool,
}

pub type ProgressMap = HashMap<Uuid, InstallProgressView>;

#[derive(Clone, Debug, Default)]
pub struct LocalMetadataView {
    pub auth_provider: Option<AuthProviderConfig>,
    pub default_xmx_mb: Option<u64>,
}

#[derive(Clone, Debug, Default)]
pub struct InstanceUserSettingsView {
    pub selected_account: Option<AccountKey>,
    pub account_override: Option<AccountKey>,
    pub xmx_mb: Option<u64>,
    pub jvm_flags: Option<Arc<str>>,
}

pub struct InstanceLiveState<'a> {
    pub installing: &'a ProgressMap,
    pub install_errors: &'a HashMap<Uuid, Arc<str>>,
    pub installed_overrides: &'a HashSet<Uuid>,
    pub launching: &'a HashSet<Uuid>,
    pub running: &'a HashSet<Uuid>,
    pub launch_errors: &'a HashMap<Uuid, Arc<str>>,
}

pub struct InstanceViewBuildInput<'a> {
    pub local_instances: &'a [LocalInstance],
    pub catalogs: &'a HashMap<Url, BackendCatalogState>,
    pub live_state: InstanceLiveState<'a>,
    pub local_metadata: &'a HashMap<Uuid, LocalMetadataView>,
    pub user_settings: &'a HashMap<Uuid, InstanceUserSettingsView>,
    pub accounts: &'a [AccountView],
}

pub fn build_instance_views(input: &InstanceViewBuildInput<'_>) -> Vec<InstanceView> {
    let InstanceViewBuildInput {
        local_instances,
        catalogs,
        live_state,
        local_metadata,
        user_settings,
        accounts,
    } = input;
    let InstanceLiveState {
        installing,
        install_errors,
        installed_overrides,
        launching,
        running,
        launch_errors,
    } = live_state;
    let mut views = Vec::new();
    let fetched_manifests = fetched_manifests(catalogs);
    let mut covered_remote_keys = HashSet::new();

    for local in *local_instances {
        let mut status = base_local_status(
            local,
            installing,
            install_errors,
            launching,
            running,
            launch_errors,
        );
        let mut orphaned = false;
        let mut manifest_auth_provider = None;
        let (display_name, origin) = match &local.source {
            Some(source) => {
                let manifest = fetched_manifests.get(&source.manifest_url);
                let remote = manifest.and_then(|manifest| {
                    manifest
                        .instances
                        .iter()
                        .find(|entry| entry.name == source.name_in_manifest)
                });

                match (manifest, remote) {
                    (Some(_), Some(remote)) => {
                        manifest_auth_provider = remote.auth_backend.clone();
                        covered_remote_keys.insert(remote_key(&source.manifest_url, &remote.name));
                        if status == InstanceLiveStatus::Installed
                            && local.last_synced_sha1.as_deref() != Some(remote.sha1.as_str())
                        {
                            status = InstanceLiveStatus::Outdated;
                        }
                        (
                            Arc::<str>::from(remote.name.clone()),
                            InstanceOrigin::Backend {
                                url: source.manifest_url.clone(),
                            },
                        )
                    }
                    (Some(_), None) => {
                        orphaned = true;
                        if status == InstanceLiveStatus::Installed {
                            status = InstanceLiveStatus::OrphanedFromBackend;
                        }
                        (
                            Arc::<str>::from(source.name_in_manifest.clone()),
                            InstanceOrigin::Backend {
                                url: source.manifest_url.clone(),
                            },
                        )
                    }
                    (None, _) => (
                        Arc::<str>::from(source.name_in_manifest.clone()),
                        InstanceOrigin::Backend {
                            url: source.manifest_url.clone(),
                        },
                    ),
                }
            }
            None => (
                Arc::<str>::from(local.dir_name.clone()),
                InstanceOrigin::Local,
            ),
        };

        let metadata = local_metadata.get(&local.id).cloned().unwrap_or_default();
        let settings = user_settings.get(&local.id).cloned().unwrap_or_default();
        let auth_provider = manifest_auth_provider.or(metadata.auth_provider);
        let has_required_account = auth_provider
            .as_ref()
            .is_none_or(|required| accounts.iter().any(|account| &account.provider == required));
        let effective_account = effective_account(
            &auth_provider,
            &settings.selected_account,
            &settings.account_override,
            accounts,
        );
        let launch_blocked_reason = launch_blocked_reason(
            &auth_provider,
            &settings.selected_account,
            &settings.account_override,
            effective_account.is_some(),
        );
        views.push(InstanceView {
            id: local.id,
            display_name,
            dir_name: Arc::<str>::from(local.dir_name.clone()),
            origin,
            status,
            locally_installed: true,
            orphaned,
            auth_provider,
            default_xmx_mb: metadata.default_xmx_mb,
            selected_account: settings.selected_account.clone(),
            account_override: settings.account_override.clone(),
            has_required_account,
            launch_blocked_reason,
            effective_account_username: effective_account
                .as_ref()
                .map(|account| account.data.user_info.username.clone().into()),
            effective_auth_provider: effective_account.map(|account| account.provider.clone()),
            effective_xmx_mb: settings.xmx_mb.or(metadata.default_xmx_mb),
            jvm_flags: settings.jvm_flags,
        });
    }

    for (url, manifest) in fetched_manifests {
        for entry in &manifest.instances {
            if covered_remote_keys.contains(&remote_key(url, &entry.name)) {
                continue;
            }

            let id = remote_entry_id(url, &entry.name);
            let status = if let Some(progress) = installing.get(&id) {
                InstanceLiveStatus::Installing {
                    stage: progress.stage,
                    current: progress.current,
                    total: progress.total,
                    message: progress.message.clone(),
                    show_bar: progress.show_bar,
                }
            } else if let Some(error) = install_errors.get(&id) {
                InstanceLiveStatus::InstallFailed(error.clone())
            } else if installed_overrides.contains(&id) {
                InstanceLiveStatus::Installed
            } else {
                InstanceLiveStatus::NotInstalled
            };

            let settings = user_settings.get(&id).cloned().unwrap_or_default();
            let effective_account = effective_account(
                &entry.auth_backend,
                &settings.selected_account,
                &settings.account_override,
                accounts,
            );
            views.push(InstanceView {
                id,
                display_name: Arc::<str>::from(entry.name.clone()),
                dir_name: Arc::<str>::from(entry.name.clone()),
                origin: InstanceOrigin::Backend { url: url.clone() },
                status,
                locally_installed: false,
                orphaned: false,
                auth_provider: entry.auth_backend.clone(),
                default_xmx_mb: None,
                selected_account: settings.selected_account.clone(),
                account_override: settings.account_override.clone(),
                has_required_account: entry.auth_backend.as_ref().is_none_or(|required| {
                    accounts.iter().any(|account| &account.provider == required)
                }),
                launch_blocked_reason: launch_blocked_reason(
                    &entry.auth_backend,
                    &settings.selected_account,
                    &settings.account_override,
                    effective_account.is_some(),
                ),
                effective_account_username: effective_account
                    .as_ref()
                    .map(|account| account.data.user_info.username.clone().into()),
                effective_auth_provider: effective_account
                    .map(|account| account.provider.clone())
                    .or(entry.auth_backend.clone()),
                effective_xmx_mb: settings.xmx_mb,
                jvm_flags: settings.jvm_flags,
            });
        }
    }

    views.sort_by(|a, b| {
        section_key(a)
            .cmp(&section_key(b))
            .then_with(|| a.display_name.cmp(&b.display_name))
            .then_with(|| a.dir_name.cmp(&b.dir_name))
            .then_with(|| a.id.cmp(&b.id))
    });
    views
}

pub fn remote_entry_id(url: &Url, name: &str) -> Uuid {
    uuid_from_seed(&format!("remote:{}:{name}", url.as_str()))
}

fn fetched_manifests(
    catalogs: &HashMap<Url, BackendCatalogState>,
) -> HashMap<&Url, Arc<InstanceManifest>> {
    catalogs
        .iter()
        .filter_map(|(url, state)| match state {
            BackendCatalogState::Fetched(manifest) => Some((url, manifest.clone())),
            _ => None,
        })
        .collect()
}

fn base_local_status(
    local: &LocalInstance,
    installing: &ProgressMap,
    install_errors: &HashMap<Uuid, Arc<str>>,
    launching: &HashSet<Uuid>,
    running: &HashSet<Uuid>,
    launch_errors: &HashMap<Uuid, Arc<str>>,
) -> InstanceLiveStatus {
    if let Some(progress) = installing.get(&local.id) {
        InstanceLiveStatus::Installing {
            stage: progress.stage,
            current: progress.current,
            total: progress.total,
            message: progress.message.clone(),
            show_bar: progress.show_bar,
        }
    } else if launching.contains(&local.id) {
        InstanceLiveStatus::Launching
    } else if running.contains(&local.id) {
        InstanceLiveStatus::Running
    } else if let Some(error) = install_errors.get(&local.id) {
        InstanceLiveStatus::InstallFailed(error.clone())
    } else if let Some(error) = launch_errors.get(&local.id) {
        InstanceLiveStatus::LaunchFailed(error.clone())
    } else {
        InstanceLiveStatus::Installed
    }
}

struct EffectiveAccount<'a> {
    provider: &'a AuthProviderConfig,
    data: &'a launcher_auth::AccountData,
}

fn effective_account<'a>(
    required_provider: &Option<AuthProviderConfig>,
    selected_account: &Option<AccountKey>,
    account_override: &Option<AccountKey>,
    accounts: &'a [AccountView],
) -> Option<EffectiveAccount<'a>> {
    if let Some(account_override) = account_override {
        return accounts
            .iter()
            .find(|account| &account.key == account_override)
            .map(|account| EffectiveAccount {
                provider: &account.provider,
                data: &account.data,
            });
    }

    if let Some(required) = required_provider {
        return selected_account
            .as_ref()
            .and_then(|selected| {
                accounts
                    .iter()
                    .find(|account| &account.key == selected && &account.provider == required)
            })
            .or_else(|| {
                accounts
                    .iter()
                    .find(|account| &account.provider == required)
            })
            .map(|account| EffectiveAccount {
                provider: &account.provider,
                data: &account.data,
            });
    }

    selected_account
        .as_ref()
        .and_then(|selected| accounts.iter().find(|account| &account.key == selected))
        .or_else(|| accounts.first())
        .map(|account| EffectiveAccount {
            provider: &account.provider,
            data: &account.data,
        })
}

fn launch_blocked_reason(
    required_provider: &Option<AuthProviderConfig>,
    _selected_account: &Option<AccountKey>,
    account_override: &Option<AccountKey>,
    has_effective_account: bool,
) -> Option<Arc<str>> {
    if account_override.is_some() || required_provider.is_none() || has_effective_account {
        return None;
    }
    Some(Arc::from(launcher_i18n::instances::launch_blocked()))
}

fn uuid_from_seed(seed: &str) -> Uuid {
    let mut first = DefaultHasher::new();
    seed.hash(&mut first);
    let first = first.finish();

    let mut second = DefaultHasher::new();
    "potato-launcher".hash(&mut second);
    seed.hash(&mut second);
    let second = second.finish();

    Uuid::from_u128(((first as u128) << 64) | (second as u128))
}

fn remote_key(url: &Url, name: &str) -> String {
    format!("{}::{name}", url.as_str())
}

fn section_key(view: &InstanceView) -> (u8, String, u8) {
    match &view.origin {
        InstanceOrigin::Local => (0, String::new(), instance_bucket(view)),
        InstanceOrigin::Backend { url } => (1, url.as_str().to_string(), instance_bucket(view)),
    }
}

fn instance_bucket(view: &InstanceView) -> u8 {
    if view.orphaned {
        2
    } else if view.locally_installed {
        0
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use instance::{manifest::InstanceManifestEntry, storage::RemoteSource};

    #[derive(Default)]
    struct TestBuildFixture {
        installing: ProgressMap,
        install_errors: HashMap<Uuid, Arc<str>>,
        installed_overrides: HashSet<Uuid>,
        launching: HashSet<Uuid>,
        running: HashSet<Uuid>,
        launch_errors: HashMap<Uuid, Arc<str>>,
        local_metadata: HashMap<Uuid, LocalMetadataView>,
        user_settings: HashMap<Uuid, InstanceUserSettingsView>,
    }

    impl TestBuildFixture {
        fn build<'a>(
            &'a self,
            local_instances: &'a [LocalInstance],
            catalogs: &'a HashMap<Url, BackendCatalogState>,
            accounts: &'a [AccountView],
        ) -> Vec<InstanceView> {
            build_instance_views(&InstanceViewBuildInput {
                local_instances,
                catalogs,
                live_state: InstanceLiveState {
                    installing: &self.installing,
                    install_errors: &self.install_errors,
                    installed_overrides: &self.installed_overrides,
                    launching: &self.launching,
                    running: &self.running,
                    launch_errors: &self.launch_errors,
                },
                local_metadata: &self.local_metadata,
                user_settings: &self.user_settings,
                accounts,
            })
        }
    }

    #[test]
    fn derives_statuses_across_multiple_backend_urls() {
        let url_a = Url::parse("https://a.example/manifest.json").unwrap();
        let url_b = Url::parse("https://b.example/manifest.json").unwrap();
        let local_only_id = Uuid::new_v4();
        let installed_id = Uuid::new_v4();
        let outdated_id = Uuid::new_v4();
        let orphaned_id = Uuid::new_v4();

        let locals = vec![
            LocalInstance {
                id: local_only_id,
                dir_name: "Local".to_string(),
                source: None,
                last_synced_sha1: None,
            },
            LocalInstance {
                id: installed_id,
                dir_name: "Installed".to_string(),
                source: Some(RemoteSource {
                    manifest_url: url_a.clone(),
                    name_in_manifest: "Installed".to_string(),
                }),
                last_synced_sha1: Some("installed-sha1".to_string()),
            },
            LocalInstance {
                id: outdated_id,
                dir_name: "Outdated".to_string(),
                source: Some(RemoteSource {
                    manifest_url: url_a.clone(),
                    name_in_manifest: "Outdated".to_string(),
                }),
                last_synced_sha1: Some("old-sha1".to_string()),
            },
            LocalInstance {
                id: orphaned_id,
                dir_name: "Orphaned".to_string(),
                source: Some(RemoteSource {
                    manifest_url: url_b.clone(),
                    name_in_manifest: "Orphaned".to_string(),
                }),
                last_synced_sha1: Some("orphaned-sha1".to_string()),
            },
        ];

        let catalogs = HashMap::from([
            (
                url_a.clone(),
                BackendCatalogState::Fetched(Arc::new(manifest([
                    ("Installed", "installed-sha1"),
                    ("Outdated", "new-sha1"),
                    ("RemoteOnly", "remote-sha1"),
                ]))),
            ),
            (
                url_b.clone(),
                BackendCatalogState::Fetched(Arc::new(manifest([("Other", "other-sha1")]))),
            ),
        ]);

        let fixture = TestBuildFixture::default();
        let views = fixture.build(&locals, &catalogs, &[]);

        assert_status(&views, local_only_id, InstanceLiveStatus::Installed);
        assert_status(&views, installed_id, InstanceLiveStatus::Installed);
        assert_status(&views, outdated_id, InstanceLiveStatus::Outdated);
        assert_status(&views, orphaned_id, InstanceLiveStatus::OrphanedFromBackend);
        assert!(views.iter().any(|view| {
            view.display_name.as_ref() == "RemoteOnly"
                && view.status == InstanceLiveStatus::NotInstalled
        }));
    }

    #[test]
    fn duplicate_display_names_from_different_backends_are_not_deduplicated() {
        let url_a = Url::parse("https://a.example/manifest.json").unwrap();
        let url_b = Url::parse("https://b.example/manifest.json").unwrap();
        let catalogs = HashMap::from([
            (
                url_a.clone(),
                BackendCatalogState::Fetched(Arc::new(manifest([("Vanilla", "a-sha1")]))),
            ),
            (
                url_b.clone(),
                BackendCatalogState::Fetched(Arc::new(manifest([("Vanilla", "b-sha1")]))),
            ),
        ]);

        let fixture = TestBuildFixture::default();
        let views = fixture.build(&[], &catalogs, &[]);

        let vanilla_views = views
            .iter()
            .filter(|view| view.display_name.as_ref() == "Vanilla")
            .collect::<Vec<_>>();
        assert_eq!(vanilla_views.len(), 2);
        assert!(
            vanilla_views
                .iter()
                .any(|view| { view.origin == InstanceOrigin::Backend { url: url_a.clone() } })
        );
        assert!(
            vanilla_views
                .iter()
                .any(|view| { view.origin == InstanceOrigin::Backend { url: url_b.clone() } })
        );
    }

    #[test]
    fn orphaned_instances_keep_their_backend_origin() {
        let url = Url::parse("https://a.example/manifest.json").unwrap();
        let id = Uuid::new_v4();
        let locals = vec![LocalInstance {
            id,
            dir_name: "Old Pack".to_string(),
            source: Some(RemoteSource {
                manifest_url: url.clone(),
                name_in_manifest: "Old Pack".to_string(),
            }),
            last_synced_sha1: Some("old".to_string()),
        }];
        let catalogs = HashMap::from([(
            url.clone(),
            BackendCatalogState::Fetched(Arc::new(manifest([("New Pack", "new")]))),
        )]);

        let fixture = TestBuildFixture::default();
        let views = fixture.build(&locals, &catalogs, &[]);
        let view = views.iter().find(|view| view.id == id).unwrap();

        assert_eq!(view.status, InstanceLiveStatus::OrphanedFromBackend);
        assert!(view.orphaned);
        assert_eq!(view.origin, InstanceOrigin::Backend { url });
    }

    #[test]
    fn orphaned_instances_stay_orphaned_while_installing() {
        let url = Url::parse("https://a.example/manifest.json").unwrap();
        let id = Uuid::new_v4();
        let locals = vec![LocalInstance {
            id,
            dir_name: "Old Pack".to_string(),
            source: Some(RemoteSource {
                manifest_url: url.clone(),
                name_in_manifest: "Old Pack".to_string(),
            }),
            last_synced_sha1: Some("old".to_string()),
        }];
        let catalogs = HashMap::from([(
            url,
            BackendCatalogState::Fetched(Arc::new(manifest([("New Pack", "new")]))),
        )]);
        let progress = HashMap::from([(
            id,
            InstallProgressView {
                stage: ProgressStage::Files,
                current: 1,
                total: 10,
                message: Arc::<str>::from(launcher_i18n::progress::downloading_files()),
                show_bar: true,
            },
        )]);

        let fixture = TestBuildFixture {
            installing: progress,
            ..Default::default()
        };
        let views = fixture.build(&locals, &catalogs, &[]);
        let view = views.iter().find(|view| view.id == id).unwrap();

        assert!(view.orphaned);
        assert!(matches!(view.status, InstanceLiveStatus::Installing { .. }));
    }

    #[test]
    fn local_metadata_fields_are_included_in_view() {
        use launcher_auth::providers::OfflineAuthProvider;

        let local = LocalInstance::new_local("Local".to_string());
        let id = local.id;
        let provider = AuthProviderConfig::Offline(OfflineAuthProvider {});
        let metadata = HashMap::from([(
            id,
            LocalMetadataView {
                auth_provider: Some(provider.clone()),
                default_xmx_mb: Some(3072),
            },
        )]);

        let fixture = TestBuildFixture {
            local_metadata: metadata.clone(),
            ..Default::default()
        };
        let views = fixture.build(&[local], &HashMap::new(), &[]);

        assert_eq!(views.len(), 1);
        assert_eq!(views[0].auth_provider, Some(provider));
        assert_eq!(views[0].default_xmx_mb, Some(3072));
    }

    #[test]
    fn missing_required_account_blocks_launch_without_override() {
        use launcher_auth::providers::{MicrosoftAuthProvider, OfflineAuthProvider};

        let local = LocalInstance::new_local("Local".to_string());
        let id = local.id;
        let required = AuthProviderConfig::Microsoft(MicrosoftAuthProvider {});
        let metadata = HashMap::from([(
            id,
            LocalMetadataView {
                auth_provider: Some(required),
                default_xmx_mb: Some(4096),
            },
        )]);
        let accounts = vec![test_account(AuthProviderConfig::Offline(
            OfflineAuthProvider {},
        ))];

        let fixture = TestBuildFixture {
            local_metadata: metadata.clone(),
            ..Default::default()
        };
        let views = fixture.build(&[local], &HashMap::new(), &accounts);

        assert!(!views[0].has_required_account);
        assert!(views[0].launch_blocked_reason.is_some());
    }

    #[test]
    fn selected_required_account_unblocks_launch_and_flows_into_view() {
        use launcher_auth::providers::MicrosoftAuthProvider;

        let local = LocalInstance::new_local("Local".to_string());
        let id = local.id;
        let required = AuthProviderConfig::Microsoft(MicrosoftAuthProvider {});
        let account = test_account(required.clone());
        let selected_key = account.key.clone();
        let metadata = HashMap::from([(
            id,
            LocalMetadataView {
                auth_provider: Some(required.clone()),
                default_xmx_mb: Some(4096),
            },
        )]);
        let settings = HashMap::from([(
            id,
            InstanceUserSettingsView {
                selected_account: Some(selected_key.clone()),
                account_override: None,
                xmx_mb: None,
                jvm_flags: None,
            },
        )]);

        let fixture = TestBuildFixture {
            local_metadata: metadata.clone(),
            user_settings: settings.clone(),
            ..Default::default()
        };
        let views = fixture.build(&[local], &HashMap::new(), &[account]);

        assert_eq!(views[0].selected_account, Some(selected_key));
        assert_eq!(views[0].effective_auth_provider, Some(required));
        assert_eq!(
            views[0].effective_account_username.as_deref(),
            Some("Tester")
        );
        assert!(views[0].launch_blocked_reason.is_none());
    }

    #[test]
    fn first_matching_required_account_is_selected_for_remote_view() {
        use launcher_auth::providers::TGAuthProvider;

        let url = Url::parse("https://a.example/manifest.json").unwrap();
        let provider = AuthProviderConfig::Telegram(TGAuthProvider {
            auth_base_url: "https://auth.example".to_string(),
        });
        let account = test_account(provider.clone());
        let entry = InstanceManifestEntry {
            name: "Remote".to_string(),
            url: Url::parse("https://example.invalid/Remote.json").unwrap(),
            sha1: "remote-sha1".to_string(),
            auth_backend: Some(provider.clone()),
        };
        let catalogs = HashMap::from([(
            url,
            BackendCatalogState::Fetched(Arc::new(InstanceManifest {
                instances: vec![entry],
            })),
        )]);

        let fixture = TestBuildFixture::default();
        let views = fixture.build(&[], &catalogs, &[account]);

        assert_eq!(views[0].selected_account, None);
        assert_eq!(views[0].effective_auth_provider, Some(provider));
        assert_eq!(
            views[0].effective_account_username.as_deref(),
            Some("Tester")
        );
        assert!(views[0].launch_blocked_reason.is_none());
    }

    #[test]
    fn remote_view_reads_saved_account_settings_before_install() {
        use launcher_auth::providers::{MicrosoftAuthProvider, OfflineAuthProvider};

        let url = Url::parse("https://a.example/manifest.json").unwrap();
        let required = AuthProviderConfig::Microsoft(MicrosoftAuthProvider {});
        let override_account = test_account(AuthProviderConfig::Offline(OfflineAuthProvider {}));
        let override_key = override_account.key.clone();
        let entry = InstanceManifestEntry {
            name: "Remote".to_string(),
            url: Url::parse("https://example.invalid/Remote.json").unwrap(),
            sha1: "remote-sha1".to_string(),
            auth_backend: Some(required),
        };
        let id = remote_entry_id(&url, &entry.name);
        let catalogs = HashMap::from([(
            url.clone(),
            BackendCatalogState::Fetched(Arc::new(InstanceManifest {
                instances: vec![entry],
            })),
        )]);
        let settings = HashMap::from([(
            id,
            InstanceUserSettingsView {
                selected_account: None,
                account_override: Some(override_key.clone()),
                xmx_mb: None,
                jvm_flags: None,
            },
        )]);

        let fixture = TestBuildFixture {
            user_settings: settings.clone(),
            ..Default::default()
        };
        let views = fixture.build(&[], &catalogs, &[override_account]);

        assert!(!views[0].locally_installed);
        assert_eq!(views[0].account_override, Some(override_key));
        assert_eq!(
            views[0].effective_account_username.as_deref(),
            Some("Tester")
        );
        assert!(views[0].launch_blocked_reason.is_none());
    }

    #[test]
    fn account_override_unblocks_launch_and_flows_into_view() {
        use launcher_auth::providers::{MicrosoftAuthProvider, OfflineAuthProvider};

        let local = LocalInstance::new_local("Local".to_string());
        let id = local.id;
        let override_account = (Uuid::new_v4(), "Tester".to_string());
        let metadata = HashMap::from([(
            id,
            LocalMetadataView {
                auth_provider: Some(AuthProviderConfig::Microsoft(MicrosoftAuthProvider {})),
                default_xmx_mb: Some(4096),
            },
        )]);
        let settings = HashMap::from([(
            id,
            InstanceUserSettingsView {
                selected_account: None,
                account_override: Some(override_account.clone()),
                xmx_mb: Some(6144),
                jvm_flags: Some(Arc::<str>::from("-Dexample=true")),
            },
        )]);
        let accounts = vec![test_account(AuthProviderConfig::Offline(
            OfflineAuthProvider {},
        ))];

        let fixture = TestBuildFixture {
            local_metadata: metadata.clone(),
            user_settings: settings.clone(),
            ..Default::default()
        };
        let views = fixture.build(&[local], &HashMap::new(), &accounts);

        assert_eq!(views[0].account_override, Some(override_account));
        assert_eq!(views[0].effective_xmx_mb, Some(6144));
        assert_eq!(views[0].jvm_flags.as_deref(), Some("-Dexample=true"));
        assert!(views[0].launch_blocked_reason.is_none());
    }

    #[test]
    fn progress_view_preserves_message_and_bar_visibility() {
        let local = LocalInstance::new_local("Local".to_string());
        let id = local.id;
        let progress = HashMap::from([(
            id,
            InstallProgressView {
                stage: ProgressStage::Metadata,
                current: 1,
                total: 1,
                message: Arc::<str>::from(launcher_i18n::progress::fetching_metadata()),
                show_bar: false,
            },
        )]);

        let fixture = TestBuildFixture {
            installing: progress,
            ..Default::default()
        };
        let views = fixture.build(&[local], &HashMap::new(), &[]);

        assert!(matches!(
            &views[0].status,
            InstanceLiveStatus::Installing { message, show_bar: false, .. }
                if message.as_ref() == launcher_i18n::progress::fetching_metadata()
        ));
    }

    fn assert_status(views: &[InstanceView], id: Uuid, expected: InstanceLiveStatus) {
        let status = &views.iter().find(|view| view.id == id).unwrap().status;
        assert_eq!(status, &expected);
    }

    fn manifest(
        entries: impl IntoIterator<Item = (&'static str, &'static str)>,
    ) -> InstanceManifest {
        InstanceManifest {
            instances: entries
                .into_iter()
                .map(|(name, sha1)| InstanceManifestEntry {
                    name: name.to_string(),
                    url: Url::parse(&format!("https://example.invalid/{name}.json")).unwrap(),
                    sha1: sha1.to_string(),
                    auth_backend: None,
                })
                .collect(),
        }
    }

    fn test_account(provider: AuthProviderConfig) -> AccountView {
        AccountView {
            key: (Uuid::new_v4(), "Tester".to_string()),
            provider,
            data: launcher_auth::AccountData {
                access_token: "token".to_string(),
                refresh_token: None,
                user_info: launcher_auth::UserInfo {
                    uuid: Uuid::new_v4(),
                    username: "Tester".to_string(),
                },
            },
            selected: false,
        }
    }
}
