package api

import "github.com/Petr1Furious/potato-launcher/backend/internal/models"

type TokenRequest struct {
	Token string `json:"token" doc:"Admin secret token"`
}

type TokenResponse struct {
	AccessToken string `json:"access_token"`
	TokenType   string `json:"token_type"`
}

type APISettings struct {
	ReplaceDownloadURLs bool `json:"replace_download_urls" doc:"Whether to replace download URLs in the client"`
}

type APIInstance struct {
	Name             string                 `json:"name" example:"survival-1.21"`
	MinecraftVersion string                 `json:"minecraft_version" example:"1.21.1"`
	ModLoader        models.LoaderType      `json:"mod_loader" example:"fabric"`
	LoaderVersion    string                 `json:"loader_version,omitempty" example:"0.15.11"`
	ContentRules     []models.ContentRule   `json:"content_rules,omitempty"`
	ModSync          models.ModSyncSettings `json:"mod_sync"`
	ResourceSync     models.ResourceSyncMode `json:"resource_sync"`
	AuthBackend      *models.AuthBackend    `json:"auth_backend,omitempty"`
	DefaultXmx       string                 `json:"default_xmx,omitempty" example:"4G"`
}

type APISpec struct {
	ReplaceDownloadURLs bool          `json:"replace_download_urls"`
	Instances           []APIInstance `json:"instances"`
}

type BuildStatusResponse struct {
	Status models.BuildStatus `json:"status"`
}
