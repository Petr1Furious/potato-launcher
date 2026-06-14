package config

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"
)

type Config struct {
	Host                     string
	Port                     int
	AdminSecretToken         string
	AdminJWTSecret           string
	AccessTokenExpireMinutes int
	AllowedOrigins           []string
	TempDir                  string
	UploadedInstancesDir     string
	SpecFile                 string
	InstanceBuilderBinary    string
	GeneratedDir             string
	WorkdirDir               string
	DownloadServerBase       string
	ResourcesURLBase         *string
	ReplaceDownloadURLs      bool
	LauncherDir              string
	LauncherName             string
}

func Load() (*Config, error) {
	cfg := &Config{
		Host:                     getEnv("HOST", "0.0.0.0"),
		Port:                     getEnvInt("PORT", 8000),
		AdminSecretToken:         os.Getenv("ADMIN_SECRET_TOKEN"),
		AdminJWTSecret:           os.Getenv("ADMIN_JWT_SECRET"),
		AccessTokenExpireMinutes: getEnvInt("ACCESS_TOKEN_EXPIRE_MINUTES", 1440),
		AllowedOrigins:           splitAndClean(getEnv("ALLOWED_ORIGINS", "*")),
		TempDir:                  getEnv("TEMP_DIR", os.TempDir()),
		UploadedInstancesDir:     getEnv("UPLOADED_INSTANCES_DIR", "/data/internal/uploaded-instances"),
		SpecFile:                 getEnv("SPEC_FILE", "/data/internal/spec.json"),
		InstanceBuilderBinary:    getEnv("INSTANCE_BUILDER_BINARY", "instance-builder"),
		GeneratedDir:             getEnv("GENERATED_DIR", "/data/generated"),
		WorkdirDir:               getEnv("WORKDIR_DIR", "/data/workdir"),
		DownloadServerBase:       os.Getenv("DOWNLOAD_SERVER_BASE"),
		ReplaceDownloadURLs:      getEnvBool("REPLACE_DOWNLOAD_URLS", false),
		LauncherDir:              getEnv("LAUNCHER_DIR", "/data/launcher"),
		LauncherName:             getEnv("LAUNCHER_NAME", "Potato Launcher"),
	}

	if resources := os.Getenv("RESOURCES_URL_BASE"); resources != "" {
		cfg.ResourcesURLBase = &resources
	}

	if cfg.AdminSecretToken == "" {
		return nil, errors.New("ADMIN_SECRET_TOKEN is required")
	}
	if cfg.AdminJWTSecret == "" {
		return nil, errors.New("ADMIN_JWT_SECRET is required")
	}
	if cfg.DownloadServerBase == "" {
		return nil, errors.New("DOWNLOAD_SERVER_BASE is required")
	}

	if cfg.ResourcesURLBase == nil {
		base := strings.TrimRight(cfg.DownloadServerBase, "/")
		defaultResources := base + "/assets/objects"
		cfg.ResourcesURLBase = &defaultResources
	}

	for _, dir := range []string{
		cfg.TempDir,
		cfg.UploadedInstancesDir,
		cfg.GeneratedDir,
		cfg.WorkdirDir,
		filepath.Dir(cfg.SpecFile),
	} {
		if err := ensureDir(dir); err != nil {
			return nil, err
		}
	}

	var err error
	if cfg.TempDir, err = filepath.Abs(cfg.TempDir); err != nil {
		return nil, err
	}
	if cfg.UploadedInstancesDir, err = filepath.Abs(cfg.UploadedInstancesDir); err != nil {
		return nil, err
	}
	if cfg.SpecFile, err = filepath.Abs(cfg.SpecFile); err != nil {
		return nil, err
	}
	if cfg.GeneratedDir, err = filepath.Abs(cfg.GeneratedDir); err != nil {
		return nil, err
	}
	if cfg.WorkdirDir, err = filepath.Abs(cfg.WorkdirDir); err != nil {
		return nil, err
	}

	return cfg, nil
}

func (c *Config) Address() string {
	return fmt.Sprintf("%s:%d", c.Host, c.Port)
}

func ensureDir(path string) error {
	if path == "" {
		return errors.New("path must not be empty")
	}
	return os.MkdirAll(path, 0o755)
}

func getEnv(key, def string) string {
	if val := os.Getenv(key); val != "" {
		return val
	}
	return def
}

func getEnvInt(key string, def int) int {
	if val := os.Getenv(key); val != "" {
		if parsed, err := strconv.Atoi(val); err == nil {
			return parsed
		}
	}
	return def
}

func getEnvBool(key string, def bool) bool {
	if val := os.Getenv(key); val != "" {
		switch strings.ToLower(val) {
		case "1", "true", "yes", "on":
			return true
		case "0", "false", "no", "off":
			return false
		}
	}
	return def
}

func splitAndClean(v string) []string {
	parts := strings.Split(v, ",")
	out := make([]string, 0, len(parts))
	for _, p := range parts {
		if trimmed := strings.TrimSpace(p); trimmed != "" {
			out = append(out, trimmed)
		}
	}
	if len(out) == 0 {
		return []string{"*"}
	}
	return out
}
