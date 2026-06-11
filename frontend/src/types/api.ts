export enum LoaderType {
  VANILLA = 'vanilla',
  FORGE = 'forge',
  FABRIC = 'fabric',
  NEOFORGE = 'neoforge',
}

export enum AuthType {
  OFFLINE = 'offline',
  MOJANG = 'mojang',
  TELEGRAM = 'telegram',
  ELY_BY = 'ely.by',
}

export enum ApplyOn {
  UPDATE = 'update',
  ALWAYS = 'always',
}

export enum ContentRuleType {
  FILE = 'file',
  DIRECTORY = 'directory',
  CONFIG_OPTIONS = 'config_options',
}

export enum ConfigType {
  JSON = 'json',
  YAML = 'yaml',
  TOML = 'toml',
  PROPERTIES = 'properties',
}

export enum ModSyncMode {
  DELTA = 'delta',
  MIRROR = 'mirror',
  MIRROR_FAST = 'mirror_fast',
}

export enum ResourceSyncMode {
  ON_UPDATE = 'on_update',
  ALWAYS = 'always',
  ALWAYS_FAST = 'always_fast',
}

export interface AuthBackend {
  type: AuthType;
  auth_base_url?: string;
  client_id?: string;
  client_secret?: string;
}

export type ConfigOptionKey = string | (string | number)[];

export interface ConfigOption {
  key: ConfigOptionKey;
  value: unknown;
}

export interface ContentRule {
  path: string;
  type: ContentRuleType;
  apply_on?: ApplyOn;
  overwrite?: boolean;
  delete_extra?: boolean;
  skip_if_dir_exists?: boolean;
  config_type?: ConfigType;
  options?: ConfigOption[];
}

export interface OptionalModSet {
  id: string;
  display_name: string;
  enabled_by_default?: boolean;
  mod_ids: string[];
}

export interface ModSyncSettings {
  mode: ModSyncMode;
  required?: string[];
  blocked?: string[];
  optional_sets?: OptionalModSet[];
}

export interface InstanceResponse {
  name: string;
  minecraft_version: string;
  mod_loader: LoaderType;
  loader_version?: string;
  auth_backend: AuthBackend;
  content_rules?: ContentRule[];
  mod_sync: ModSyncSettings;
  resource_sync: ResourceSyncMode;
  default_xmx?: string;
}

export interface InstanceBase {
  name: string;
  minecraft_version: string;
  mod_loader: LoaderType;
  loader_version?: string;
  auth_backend: AuthBackend;
  content_rules?: ContentRule[];
  mod_sync: ModSyncSettings;
  resource_sync: ResourceSyncMode;
  default_xmx?: string;
}

export interface Settings {
  replace_download_urls: boolean;
}
