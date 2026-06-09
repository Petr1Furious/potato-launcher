use std::{
    fs,
    path::{Path, PathBuf},
};

use relative_path::{RelativePath, RelativePathBuf};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use url::Url;

const INSTANCES_DIR_NAME: &str = "instances";
const VERSIONS_DIR_NAME: &str = "versions";
const VERSIONS_REPLACED_DIR_NAME: &str = "versions_replaced";
const MINECRAFT_DIR_NAME: &str = "minecraft";
const MODS_DIR_NAME: &str = "mods";
const OPTIONAL_MODS_DIR_NAME: &str = "optional_mods";
const META_FILE_NAME: &str = "meta.json";
const LOCAL_INSTANCE_FILE_NAME: &str = "local_instance.json";
const INSTANCE_SETTINGS_FILE_NAME: &str = "settings.json";
const AUTH_DATA_FILE_NAME: &str = "auth_data.json";
const JAVA_DIR_NAME: &str = "java";
const LOGS_DIR_NAME: &str = "logs";
const LIBRARIES_DIR_NAME: &str = "libraries";
const NATIVES_DIR_NAME: &str = "natives";
const INDEXES_DIR_NAME: &str = "indexes";
const OBJECTS_DIR_NAME: &str = "objects";
const ASSETS_DIR_NAME: &str = "assets";

lazy_static::lazy_static! {
    pub static ref MOJANG_RESOURCES_URL_BASE: Url = Url::parse("https://resources.download.minecraft.net/").unwrap();
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Rel(RelativePathBuf);

impl Rel {
    pub fn new(p: impl Into<RelativePathBuf>) -> Self {
        Self(p.into())
    }

    pub fn join(&self, seg: impl AsRef<str>) -> Self {
        let mut p = self.0.clone();
        p.push(seg.as_ref());
        Self(p)
    }

    pub fn parent(&self) -> Option<Self> {
        self.0.parent().map(|p| Self(p.into()))
    }

    pub fn to_fs(&self, base: &Path) -> PathBuf {
        self.0.to_path(base)
    }

    pub fn to_url(&self, base: &Url) -> Url {
        let base = ensure_trailing_slash(base);
        base.join(self.0.as_str()).expect("valid url join")
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

fn ensure_trailing_slash(base: &Url) -> Url {
    let mut base = base.clone();
    if !base.path().ends_with('/') {
        let new_path = format!("{}/", base.path());
        base.set_path(&new_path);
    }
    base
}

macro_rules! path_type {
    ($name:ident, dir) => {
        #[derive(Clone, Debug, PartialEq, Eq, Hash)]
        #[repr(transparent)]
        pub struct $name(Rel);

        impl $name {
            pub fn rel(&self) -> &Rel {
                &self.0
            }

            pub fn into_rel(self) -> Rel {
                self.0
            }

            pub fn to_fs_safe(&self, data_dir: &DataDir) -> PathBuf {
                let path = self.0.to_fs(data_dir.as_path());
                ensure_dir(&path);
                path
            }

            pub fn to_fs(&self, data_dir: &DataDir) -> PathBuf {
                self.0.to_fs(data_dir.as_path())
            }

            pub fn ensure_dir(&self, data_dir: &DataDir) {
                let path = self.0.to_fs(data_dir.as_path());
                ensure_dir(&path);
            }

            pub fn to_url(&self, base_url: &BaseUrl) -> Url {
                self.0.to_url(base_url.as_url())
            }
        }
    };
    ($name:ident, file) => {
        #[derive(Clone, Debug, PartialEq, Eq, Hash)]
        #[repr(transparent)]
        pub struct $name(Rel);

        impl $name {
            pub fn rel(&self) -> &Rel {
                &self.0
            }

            pub fn into_rel(self) -> Rel {
                self.0
            }

            pub fn to_fs_safe(&self, data_dir: &DataDir) -> PathBuf {
                let path = self.0.to_fs(data_dir.as_path());
                ensure_parent(&path);
                path
            }

            pub fn to_fs(&self, data_dir: &DataDir) -> PathBuf {
                self.0.to_fs(data_dir.as_path())
            }

            pub fn ensure_parent(&self, data_dir: &DataDir) {
                let path = self.0.to_fs(data_dir.as_path());
                ensure_parent(&path);
            }

            pub fn to_url(&self, base_url: &BaseUrl) -> Url {
                self.0.to_url(base_url.as_url())
            }
        }
    };
}

path_type!(InstancesDir, dir);
path_type!(InstanceDir, dir);
path_type!(MinecraftDir, dir);
path_type!(JavaDir, dir);
path_type!(JavaVersionDir, dir);
path_type!(LogsDir, dir);
path_type!(LibrariesDir, dir);
path_type!(NativesDir, dir);
path_type!(VersionsDir, dir);
path_type!(VersionsReplacedDir, dir);
path_type!(AssetsDir, dir);
path_type!(AssetsObjectsDir, dir);
path_type!(ModsDir, dir);
path_type!(OptionalModsDir, dir);
path_type!(InstanceMetaPath, file);
path_type!(LocalInstanceDescriptorPath, file);
path_type!(InstanceSettingsPath, file);
path_type!(JavaBinPath, file);
path_type!(AuthDataPath, file);
path_type!(MetadataPath, file);
path_type!(LibraryPath, file);
path_type!(NativePath, file);
path_type!(ClientJarPath, file);
path_type!(AssetIndexPath, file);
path_type!(AssetObjectPath, file);
path_type!(InstanceObjectPath, file);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DataDir(PathBuf);

impl DataDir {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self(data_dir.into())
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize)]
pub struct BaseUrl(Url);

impl BaseUrl {
    pub fn new(base_url: Url) -> Self {
        Self(base_url)
    }

    pub fn as_url(&self) -> &Url {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct InstanceDirFS {
    data_dir: DataDir,
    rel: InstanceDir,
}

impl InstanceDirFS {
    pub fn new(data_dir: DataDir, rel: InstanceDir) -> Self {
        Self { data_dir, rel }
    }

    pub fn data_dir(&self) -> &DataDir {
        &self.data_dir
    }

    pub fn rel(&self) -> &InstanceDir {
        &self.rel
    }

    pub fn to_fs(&self) -> PathBuf {
        self.rel.to_fs(&self.data_dir)
    }

    pub fn to_fs_safe(&self) -> PathBuf {
        self.rel.to_fs_safe(&self.data_dir)
    }

    pub fn ensure_dir(&self) {
        self.rel.ensure_dir(&self.data_dir);
    }

    pub fn minecraft_dir(&self) -> PathBuf {
        self.rel.minecraft_dir().to_fs(&self.data_dir)
    }

    pub fn mods_dir(&self) -> PathBuf {
        self.rel.minecraft_dir().mods_dir().to_fs(&self.data_dir)
    }

    pub fn optional_mods_dir(&self) -> PathBuf {
        self.rel.optional_mods_dir().to_fs(&self.data_dir)
    }

    pub fn meta_path(&self) -> PathBuf {
        self.rel.meta_path().to_fs(&self.data_dir)
    }

    pub fn local_instance_descriptor_path(&self) -> PathBuf {
        self.rel
            .local_instance_descriptor_path()
            .to_fs(&self.data_dir)
    }

    pub fn settings_path(&self) -> PathBuf {
        self.rel.settings_path().to_fs(&self.data_dir)
    }
}

fn ensure_dir(path: &Path) {
    if let Err(err) = fs::create_dir_all(path) {
        panic!("Failed to create directory {}: {err}", path.display());
    }
}

fn ensure_parent(path: &Path) {
    let parent = path.parent().expect("Path should have a parent directory");
    ensure_dir(parent);
}

impl InstancesDir {
    pub fn root() -> Self {
        Self(Rel::new(INSTANCES_DIR_NAME))
    }

    pub fn instance_dir(&self, dir_name: &str) -> InstanceDir {
        InstanceDir(self.0.join(dir_name))
    }
}

impl InstanceDir {
    pub fn with_data_dir(&self, data_dir: DataDir) -> InstanceDirFS {
        InstanceDirFS::new(data_dir, self.clone())
    }

    pub fn minecraft_dir(&self) -> MinecraftDir {
        MinecraftDir(self.0.join(MINECRAFT_DIR_NAME))
    }

    pub fn optional_mods_dir(&self) -> OptionalModsDir {
        OptionalModsDir(self.0.join(OPTIONAL_MODS_DIR_NAME))
    }

    pub fn meta_path(&self) -> InstanceMetaPath {
        InstanceMetaPath(self.0.join(META_FILE_NAME))
    }

    pub fn local_instance_descriptor_path(&self) -> LocalInstanceDescriptorPath {
        LocalInstanceDescriptorPath(self.0.join(LOCAL_INSTANCE_FILE_NAME))
    }

    pub fn settings_path(&self) -> InstanceSettingsPath {
        InstanceSettingsPath(self.0.join(INSTANCE_SETTINGS_FILE_NAME))
    }
}

impl MinecraftDir {
    pub fn instance_object_path(&self, object_path: &RelativePath) -> InstanceObjectPath {
        InstanceObjectPath(self.0.join(object_path))
    }

    pub fn mods_dir(&self) -> ModsDir {
        ModsDir(self.0.join(MODS_DIR_NAME))
    }
}

impl ModsDir {
    pub fn name() -> &'static str {
        MODS_DIR_NAME
    }

    pub fn mod_jar_path(&self, filename: &str) -> InstanceObjectPath {
        InstanceObjectPath(self.0.join(filename))
    }

    pub fn to_fs_at(&self, minecraft_dir: &Path) -> PathBuf {
        self.0.to_fs(minecraft_dir)
    }
}

impl OptionalModsDir {
    pub fn mod_jar_path(&self, filename: &str) -> InstanceObjectPath {
        InstanceObjectPath(self.0.join(filename))
    }
}

impl InstanceObjectPath {
    pub fn to_relative_path(&self) -> &RelativePath {
        &self.0.0
    }
}

impl AuthDataPath {
    pub fn root() -> Self {
        Self(Rel::new(AUTH_DATA_FILE_NAME))
    }
}

impl JavaDir {
    pub fn root() -> Self {
        Self(Rel::new(JAVA_DIR_NAME))
    }

    pub fn java_version_dir(&self, version: &str) -> JavaVersionDir {
        JavaVersionDir(self.0.join(version))
    }
}

impl JavaVersionDir {
    pub fn bin_path(&self, binary_name: &str) -> JavaBinPath {
        JavaBinPath(self.0.join("bin").join(binary_name))
    }
}

impl LogsDir {
    pub fn root() -> Self {
        Self(Rel::new(LOGS_DIR_NAME))
    }
}

impl LibrariesDir {
    pub fn root() -> Self {
        Self(Rel::new(LIBRARIES_DIR_NAME))
    }

    pub fn library_path(&self, rel_library_path: &RelativePath) -> LibraryPath {
        LibraryPath(self.0.join(rel_library_path))
    }
}

#[derive(thiserror::Error, Debug)]
pub enum LibraryError {
    #[error("Invalid library path: {path}")]
    InvalidLibraryPath { path: RelativePathBuf },
}

impl LibraryPath {
    pub fn native_path(
        &self,
        native_name: &str,
        filename: &str,
    ) -> Result<NativePath, LibraryError> {
        Ok(NativePath(
            self.0
                .parent()
                .ok_or(LibraryError::InvalidLibraryPath {
                    path: self.0.0.clone(),
                })?
                .join(native_name)
                .join(filename),
        ))
    }
}

impl NativesDir {
    pub fn for_id(id: &str) -> Self {
        Self(Rel::new(NATIVES_DIR_NAME).join(id))
    }
}

impl VersionsDir {
    pub fn root() -> Self {
        Self(Rel::new(VERSIONS_DIR_NAME))
    }

    pub fn metadata_path(&self, version_id: &str) -> MetadataPath {
        MetadataPath(self.0.join(version_id).join(format!("{version_id}.json")))
    }

    pub fn client_jar_path(&self, id: &str) -> ClientJarPath {
        ClientJarPath(self.0.join(id).join(format!("{id}.jar")))
    }
}

impl VersionsReplacedDir {
    pub fn root() -> Self {
        Self(Rel::new(VERSIONS_REPLACED_DIR_NAME))
    }

    pub fn metadata_path(&self, version_id: &str) -> MetadataPath {
        MetadataPath(self.0.join(version_id).join(format!("{version_id}.json")))
    }
}

impl AssetsDir {
    pub fn root() -> Self {
        Self(Rel::new(ASSETS_DIR_NAME))
    }

    pub fn new(rel: impl Into<Rel>) -> Self {
        Self(rel.into())
    }

    pub fn asset_index_path(&self, asset_index: &str) -> AssetIndexPath {
        AssetIndexPath(
            self.0
                .join(INDEXES_DIR_NAME)
                .join(format!("{asset_index}.json")),
        )
    }

    pub fn assets_object_dir(&self) -> AssetsObjectsDir {
        AssetsObjectsDir(self.0.join(OBJECTS_DIR_NAME))
    }
}

fn object_rel_path(object_hash: &str) -> RelativePathBuf {
    RelativePathBuf::from(format!("{}/{}", &object_hash[..2], object_hash))
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResourcesUrlBase(Url);

impl ResourcesUrlBase {
    pub fn as_url(&self) -> &Url {
        &self.0
    }

    pub fn object_url(&self, object_hash: &str) -> Result<Url, url::ParseError> {
        self.0.join(object_rel_path(object_hash).as_str())
    }
}

impl Default for ResourcesUrlBase {
    fn default() -> Self {
        Self(MOJANG_RESOURCES_URL_BASE.clone())
    }
}

impl Serialize for ResourcesUrlBase {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.0.as_str())
    }
}

impl<'de> Deserialize<'de> for ResourcesUrlBase {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Url::parse(&s)
            .map(ResourcesUrlBase)
            .map_err(serde::de::Error::custom)
    }
}

impl AssetsObjectsDir {
    pub fn to_resources_url_base(&self, base_url: &BaseUrl) -> ResourcesUrlBase {
        ResourcesUrlBase(ensure_trailing_slash(&self.0.to_url(base_url.as_url())))
    }

    pub fn object_path(&self, object_hash: &str) -> AssetObjectPath {
        AssetObjectPath(self.0.join(object_rel_path(object_hash)))
    }
}
