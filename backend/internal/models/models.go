package models

import (
	"encoding/json"
	"time"
)

type LoaderType string

const (
	LoaderVanilla LoaderType = "vanilla"
	LoaderForge   LoaderType = "forge"
	LoaderFabric  LoaderType = "fabric"
	LoaderNeo     LoaderType = "neoforge"
)

type AuthType string

const (
	AuthMojang   AuthType = "mojang"
	AuthTelegram AuthType = "telegram"
	AuthEly      AuthType = "ely.by"
	AuthOffline  AuthType = "offline"
)

type AuthBackend struct {
	Type         AuthType `json:"type"`
	AuthBaseURL  string   `json:"auth_base_url,omitempty"`
	ClientID     string   `json:"client_id,omitempty"`
	ClientSecret string   `json:"client_secret,omitempty"`
}

type ApplyOn string

const (
	ApplyOnUpdate ApplyOn = "update"
	ApplyOnAlways ApplyOn = "always"
)

type ContentRuleType string

const (
	ContentRuleFile          ContentRuleType = "file"
	ContentRuleDirectory     ContentRuleType = "directory"
	ContentRuleConfigOptions ContentRuleType = "config_options"
)

type ConfigType string

const (
	ConfigTypeJson       ConfigType = "json"
	ConfigTypeYaml       ConfigType = "yaml"
	ConfigTypeToml       ConfigType = "toml"
	ConfigTypeProperties ConfigType = "properties"
)

type ConfigOption struct {
	Key   json.RawMessage `json:"key"`
	Value json.RawMessage `json:"value"`
}

type ContentRule struct {
	Path      string          `json:"path"`
	ApplyOn   ApplyOn         `json:"apply_on,omitempty"`
	Overwrite *bool           `json:"overwrite,omitempty"`
	Type      ContentRuleType `json:"type"`

	DeleteExtra     *bool `json:"delete_extra,omitempty"`
	SkipIfDirExists *bool `json:"skip_if_dir_exists,omitempty"`

	ConfigType *ConfigType    `json:"config_type,omitempty"`
	Options    []ConfigOption `json:"options,omitempty"`
}

type ModSyncMode string

const (
	ModSyncDelta      ModSyncMode = "delta"
	ModSyncMirror     ModSyncMode = "mirror"
	ModSyncMirrorFast ModSyncMode = "mirror_fast"
)

type OptionalModSet struct {
	ID               string   `json:"id"`
	DisplayName      string   `json:"display_name"`
	EnabledByDefault bool     `json:"enabled_by_default,omitempty"`
	ModIDs           []string `json:"mod_ids"`
}

type ModSyncSettings struct {
	Mode         ModSyncMode      `json:"mode"`
	Required     []string         `json:"required,omitempty"`
	Blocked      []string         `json:"blocked,omitempty"`
	OptionalSets []OptionalModSet `json:"optional_sets,omitempty"`
}

type ResourceSyncMode string

const (
	ResourceSyncOnUpdate   ResourceSyncMode = "on_update"
	ResourceSyncAlways     ResourceSyncMode = "always"
	ResourceSyncAlwaysFast ResourceSyncMode = "always_fast"
)

type BuilderInstance struct {
	Name             string           `json:"name"`
	MinecraftVersion string           `json:"minecraft_version"`
	ModLoader        LoaderType       `json:"mod_loader"`
	LoaderVersion    string           `json:"loader_version,omitempty"`
	SourceRoot       string           `json:"source_root,omitempty"`
	ContentRules     []ContentRule    `json:"content_rules,omitempty"`
	ModSync          ModSyncSettings  `json:"mod_sync"`
	ResourceSync     ResourceSyncMode `json:"resource_sync"`
	AuthBackend      *AuthBackend     `json:"auth_backend,omitempty"`
	DefaultXmx       string           `json:"default_xmx,omitempty"`
}

type BuilderSpec struct {
	DownloadServerBase  string            `json:"download_server_base"`
	ResourcesURLBase    *string           `json:"resources_url_base,omitempty"`
	ReplaceDownloadURLs bool              `json:"replace_download_urls"`
	Instances           []BuilderInstance `json:"instances"`
}

type BuildStatus string

const (
	BuildRunning BuildStatus = "running"
	BuildIdle    BuildStatus = "idle"
)

type JWTClaims struct {
	Sub string `json:"sub"`
	Exp time.Time
}
