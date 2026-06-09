use std::path::PathBuf;

use launcher_auth::providers::AuthProviderConfig;
use relative_path::RelativePath;
use url::Url;
use utils::paths::{BaseUrl, DataDir, LibrariesDir};

use crate::{
    instance_metadata::{InstanceMetadata, InstanceMetadataError},
    version_metadata::Library,
};

pub const AUTHLIB_INJECTOR_GAV: &str = "moe.yushi:authlibinjector:1.2.7";
pub const AUTHLIB_INJECTOR_REL_PATH: &str =
    "moe/yushi/authlibinjector/1.2.7/authlibinjector-1.2.7.jar";
pub const AUTHLIB_INJECTOR_UPSTREAM_URL: &str = "https://github.com/yushijinhun/authlib-injector/releases/download/v1.2.7/authlib-injector-1.2.7.jar";
/// SHA1 of authlib-injector v1.2.7 from the upstream GitHub release.
pub const AUTHLIB_INJECTOR_UPSTREAM_SHA1: &str = "9a401b2cdb97bf49e5c447dcfb0325979168b672";
/// Size in bytes of authlib-injector v1.2.7 from the upstream GitHub release.
pub const AUTHLIB_INJECTOR_UPSTREAM_SIZE: u64 = 344477;

lazy_static::lazy_static! {
    pub static ref DEFAULT_AUTHLIB_INJECTOR_LIBRARY: Library = Library::from_download(
        AUTHLIB_INJECTOR_GAV.to_string(),
        Url::parse(AUTHLIB_INJECTOR_UPSTREAM_URL).expect("valid authlib-injector upstream URL"),
        AUTHLIB_INJECTOR_UPSTREAM_SHA1.to_string(),
        AUTHLIB_INJECTOR_UPSTREAM_SIZE,
    );
}

pub fn default_authlib_injector_library() -> Library {
    DEFAULT_AUTHLIB_INJECTOR_LIBRARY.clone()
}

pub async fn mirror_authlib_injector_library(
    data_dir: &DataDir,
    download_server_base: &BaseUrl,
) -> Result<Library, std::io::Error> {
    let path = LibrariesDir::root()
        .library_path(RelativePath::new(AUTHLIB_INJECTOR_REL_PATH))
        .to_fs(data_dir);
    let size = path.metadata()?.len();
    let sha1 = utils::files::hash_file(&path).await?;
    let url = LibrariesDir::root()
        .library_path(RelativePath::new(AUTHLIB_INJECTOR_REL_PATH))
        .to_url(download_server_base);
    Ok(Library::from_download(
        AUTHLIB_INJECTOR_GAV.to_string(),
        url,
        sha1,
        size,
    ))
}

impl InstanceMetadata {
    pub fn authlib_injector_path(
        &self,
        data_dir: &DataDir,
        provider: &AuthProviderConfig,
    ) -> Result<Option<PathBuf>, InstanceMetadataError> {
        if provider.get_injector_url().is_none() {
            return Ok(None);
        }
        Ok(self.authlib_injector.get_artifact_path(data_dir)?)
    }
}
