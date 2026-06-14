package services

import (
	"context"
	"encoding/json"
	"encoding/xml"
	"errors"
	"fmt"
	"io"
	"net/http"
	"slices"
	"strings"
	"time"

	"github.com/Petr1Furious/potato-launcher/backend/internal/models"
)

const (
	mojangManifestURL   = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json"
	fabricMetaBaseURL   = "https://meta.fabricmc.net/v2/versions/loader/"
	forgeMetadataURL    = "https://files.minecraftforge.net/net/minecraftforge/forge/maven-metadata.json"
	neoforgeMetadataURL = "https://maven.neoforged.net/releases/net/neoforged/neoforge/maven-metadata.xml"
)

var httpClient = &http.Client{
	Timeout: 10 * time.Second,
}

func GetVanillaVersions(ctx context.Context, versionType string) ([]string, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, mojangManifestURL, nil)
	if err != nil {
		return nil, err
	}
	resp, err := httpClient.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("mojang manifest error: %s", resp.Status)
	}
	var payload struct {
		Versions []struct {
			ID   string `json:"id"`
			Type string `json:"type"`
		} `json:"versions"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&payload); err != nil {
		return nil, err
	}
	out := make([]string, 0, len(payload.Versions))
	for _, v := range payload.Versions {
		if versionType == "" || strings.EqualFold(v.Type, versionType) {
			out = append(out, v.ID)
		}
	}
	return out, nil
}

func GetLoadersForVersion(ctx context.Context, version string) ([]models.LoaderType, error) {
	vanilla, err := GetVanillaVersions(ctx, "")
	if err != nil {
		return nil, err
	}
	loaders := make([]models.LoaderType, 0, 4)
	if slices.Contains(vanilla, version) {
		loaders = append(loaders, models.LoaderVanilla)
	}
	if ok, _ := fabricHasLoader(ctx, version); ok {
		loaders = append(loaders, models.LoaderFabric)
	}
	if ok, _ := forgeHasLoader(ctx, version); ok {
		loaders = append(loaders, models.LoaderForge)
	}
	if ok, _ := neoforgeHasLoader(ctx, version); ok {
		loaders = append(loaders, models.LoaderNeo)
	}
	return loaders, nil
}

func GetLoaderVersions(ctx context.Context, version string, loader models.LoaderType) ([]string, error) {
	switch loader {
	case models.LoaderVanilla:
		return []string{version}, nil
	case models.LoaderFabric:
		return getFabricLoaderVersions(ctx, version)
	case models.LoaderForge:
		return getForgeLoaderVersions(ctx, version)
	case models.LoaderNeo:
		return getNeoforgeLoaderVersions(ctx, version)
	default:
		return nil, errors.New("unknown loader")
	}
}

func fabricHasLoader(ctx context.Context, version string) (bool, error) {
	versions, err := getFabricLoaderVersions(ctx, version)
	return len(versions) > 0, err
}

func getFabricLoaderVersions(ctx context.Context, version string) ([]string, error) {
	url := fabricMetaBaseURL + version
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return nil, err
	}
	resp, err := httpClient.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	if resp.StatusCode == http.StatusNotFound || resp.StatusCode == http.StatusBadRequest {
		return []string{}, nil
	}
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("fabric meta error: %s", resp.Status)
	}
	var payload []struct {
		Loader struct {
			Version string `json:"version"`
		} `json:"loader"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&payload); err != nil {
		return nil, err
	}
	seen := map[string]struct{}{}
	out := make([]string, 0, len(payload))
	for _, item := range payload {
		if item.Loader.Version == "" {
			continue
		}
		if _, ok := seen[item.Loader.Version]; !ok {
			seen[item.Loader.Version] = struct{}{}
			out = append(out, item.Loader.Version)
		}
	}
	return out, nil
}

func forgeHasLoader(ctx context.Context, version string) (bool, error) {
	data, err := fetchForgeMetadata(ctx)
	if err != nil {
		return false, err
	}
	_, ok := data[version]
	return ok, nil
}

func getForgeLoaderVersions(ctx context.Context, version string) ([]string, error) {
	data, err := fetchForgeMetadata(ctx)
	if err != nil {
		return nil, err
	}
	items := data[version]
	out := make([]string, 0, len(items))
	prefix := version + "-"
	for _, item := range items {
		if strings.HasPrefix(item, prefix) {
			out = append(out, strings.TrimPrefix(item, prefix))
		} else {
			out = append(out, item)
		}
	}
	slices.SortFunc(out, func(a, b string) int {
		if a == b {
			return 0
		}
		return strings.Compare(b, a)
	})
	return out, nil
}

func fetchForgeMetadata(ctx context.Context) (map[string][]string, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, forgeMetadataURL, nil)
	if err != nil {
		return nil, err
	}
	resp, err := httpClient.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("forge metadata error: %s", resp.Status)
	}
	var payload map[string][]string
	if err := json.NewDecoder(resp.Body).Decode(&payload); err != nil {
		return nil, err
	}
	return payload, nil
}

func neoforgeHasLoader(ctx context.Context, version string) (bool, error) {
	versions, err := getNeoforgeLoaderVersions(ctx, version)
	return len(versions) > 0, err
}

func getNeoforgeLoaderVersions(ctx context.Context, version string) ([]string, error) {
	prefix := neoforgeMinecraftVersionPrefix(version)
	if len(prefix) == 0 {
		return nil, nil
	}
	items, err := fetchNeoforgeVersions(ctx)
	if err != nil {
		return nil, err
	}
	matched := make([]string, 0)
	for _, item := range items {
		if versionFragmentsStartWith(item, prefix) {
			matched = append(matched, item)
		}
	}
	slices.SortFunc(matched, func(a, b string) int {
		return compareVersionFragments(versionStringToParts(a), versionStringToParts(b))
	})
	slices.Reverse(matched)
	return matched, nil
}

func fetchNeoforgeVersions(ctx context.Context) ([]string, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, neoforgeMetadataURL, nil)
	if err != nil {
		return nil, err
	}
	resp, err := httpClient.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("neoforge metadata error: %s", resp.Status)
	}
	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, err
	}
	var payload struct {
		Versioning struct {
			Versions struct {
				Version []string `xml:"version"`
			} `xml:"versions"`
		} `xml:"versioning"`
	}
	if err := xml.Unmarshal(body, &payload); err != nil {
		return nil, err
	}
	return payload.Versioning.Versions.Version, nil
}

type versionFragmentKind int

const (
	versionFragmentNumber versionFragmentKind = iota
	versionFragmentAlpha
	versionFragmentBeta
	versionFragmentSnapshot
	versionFragmentString
)

type versionFragment struct {
	kind  versionFragmentKind
	text  string
	value int
}

func versionStringToParts(version string) []versionFragment {
	parts := strings.FieldsFunc(version, func(r rune) bool {
		return r == '.' || r == '-' || r == '+'
	})
	out := make([]versionFragment, 0, len(parts))
	for _, part := range parts {
		if part == "" {
			continue
		}
		if value, err := parseVersionNumber(part); err == nil {
			out = append(out, versionFragment{kind: versionFragmentNumber, value: value})
			continue
		}
		switch strings.ToLower(part) {
		case "alpha":
			out = append(out, versionFragment{kind: versionFragmentAlpha})
		case "beta":
			out = append(out, versionFragment{kind: versionFragmentBeta})
		case "snapshot":
			out = append(out, versionFragment{kind: versionFragmentSnapshot})
		default:
			out = append(out, versionFragment{kind: versionFragmentString, text: part})
		}
	}
	return out
}

func parseVersionNumber(part string) (int, error) {
	value := 0
	for _, ch := range part {
		if ch < '0' || ch > '9' {
			return 0, fmt.Errorf("not a number")
		}
		value = value*10 + int(ch-'0')
	}
	return value, nil
}

func neoforgeMinecraftVersionPrefix(minecraftVersion string) []versionFragment {
	parts := versionStringToParts(minecraftVersion)
	if len(parts) == 0 {
		return parts
	}

	if parts[0].kind == versionFragmentString && parts[0].text == "25w14craftmine" {
		return append([]versionFragment{{kind: versionFragmentNumber, value: 0}}, parts...)
	}

	for len(parts) < 3 {
		parts = append(parts, versionFragment{kind: versionFragmentNumber, value: 0})
	}
	if parts[0].kind == versionFragmentNumber && parts[0].value == 1 {
		parts = parts[1:]
	}
	return parts
}

func versionFragmentsStartWith(version string, prefix []versionFragment) bool {
	parts := versionStringToParts(version)
	if len(parts) < len(prefix) {
		return false
	}
	for i, fragment := range prefix {
		if !versionFragmentEqual(parts[i], fragment) {
			return false
		}
	}
	return true
}

func versionFragmentEqual(a, b versionFragment) bool {
	if a.kind != b.kind {
		return false
	}
	switch a.kind {
	case versionFragmentNumber:
		return a.value == b.value
	case versionFragmentString:
		return a.text == b.text
	default:
		return true
	}
}

func compareVersionFragments(a, b []versionFragment) int {
	maxLen := len(a)
	if len(b) > maxLen {
		maxLen = len(b)
	}
	for i := 0; i < maxLen; i++ {
		var left, right versionFragment
		if i < len(a) {
			left = a[i]
		}
		if i < len(b) {
			right = b[i]
		}
		if cmp := compareVersionFragment(left, right); cmp != 0 {
			return cmp
		}
	}
	return 0
}

func compareVersionFragment(a, b versionFragment) int {
	if a.kind != b.kind {
		return int(a.kind) - int(b.kind)
	}
	switch a.kind {
	case versionFragmentNumber:
		return a.value - b.value
	case versionFragmentString:
		return strings.Compare(a.text, b.text)
	default:
		return 0
	}
}
