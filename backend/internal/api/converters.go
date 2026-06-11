package api

import (
	"os"
	"path/filepath"
	"strings"

	"github.com/Petr1Furious/potato-launcher/backend/internal/config"
	"github.com/Petr1Furious/potato-launcher/backend/internal/models"
)

func toAPISettings(spec *models.BuilderSpec) APISettings {
	return APISettings{
		ReplaceDownloadURLs: spec.ReplaceDownloadURLs,
	}
}

func applySettingsToSpec(spec *models.BuilderSpec, settings APISettings) {
	spec.ReplaceDownloadURLs = settings.ReplaceDownloadURLs
}

func toAPIInstance(v models.BuilderInstance) APIInstance {
	return APIInstance{
		Name:             v.Name,
		MinecraftVersion: v.MinecraftVersion,
		ModLoader:        v.ModLoader,
		LoaderVersion:    v.LoaderVersion,
		DefaultXmx:       v.DefaultXmx,
		ContentRules:     v.ContentRules,
		ModSync:          v.ModSync,
		ResourceSync:     v.ResourceSync,
		AuthBackend:      v.AuthBackend,
	}
}

func getInstanceDir(cfg *config.Config, instanceName string) string {
	return filepath.Join(cfg.UploadedInstancesDir, instanceName)
}

func ensureSourceRoot(cfg *config.Config, instance *models.BuilderInstance) {
	instance.SourceRoot = filepath.ToSlash(getInstanceDir(cfg, instance.Name))
}

func ensureInstanceDir(cfg *config.Config, instanceName string) error {
	dir := getInstanceDir(cfg, instanceName)
	return os.MkdirAll(dir, 0o755)
}

func ensureAuthBackend(instance *models.BuilderInstance) {
	if instance.AuthBackend == nil {
		instance.AuthBackend = &models.AuthBackend{Type: models.AuthOffline}
	}
}

func ensureModSyncDefaults(instance *models.BuilderInstance) {
	if instance.ModSync.Mode == "" {
		instance.ModSync.Mode = models.ModSyncDelta
	}
}

func ensureResourceSyncDefault(instance *models.BuilderInstance) {
	if instance.ResourceSync == "" {
		instance.ResourceSync = models.ResourceSyncOnUpdate
	}
}

func normalizeInstance(cfg *config.Config, instance *models.BuilderInstance) error {
	instance.Name = strings.TrimSpace(instance.Name)
	if instance.Name == "" {
		return NewValidationError("name", "name is required")
	}
	instance.MinecraftVersion = strings.TrimSpace(instance.MinecraftVersion)
	if instance.MinecraftVersion == "" {
		return NewValidationError("minecraft_version", "minecraft_version is required")
	}
	if instance.ModLoader == "" {
		instance.ModLoader = models.LoaderVanilla
	}
	if instance.ModLoader != models.LoaderVanilla && strings.TrimSpace(instance.LoaderVersion) == "" {
		return NewValidationError("loader_version", "loader_version is required")
	}

	ensureSourceRoot(cfg, instance)
	ensureAuthBackend(instance)
	ensureModSyncDefaults(instance)
	ensureResourceSyncDefault(instance)
	return nil
}

func toBuilderInstance(cfg *config.Config, m APIInstance) (*models.BuilderInstance, error) {
	instance := models.BuilderInstance{
		Name:             m.Name,
		MinecraftVersion: m.MinecraftVersion,
		ModLoader:        m.ModLoader,
		LoaderVersion:    m.LoaderVersion,
		DefaultXmx:       m.DefaultXmx,
		ContentRules:     m.ContentRules,
		ModSync:          m.ModSync,
		ResourceSync:     m.ResourceSync,
		AuthBackend:      m.AuthBackend,
	}
	if err := normalizeInstance(cfg, &instance); err != nil {
		return nil, err
	}
	return &instance, nil
}
