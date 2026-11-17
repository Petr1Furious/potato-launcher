//! Offline end-to-end tests of pull, push and status against temp folders.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use crate::{ctx::Ctx, index::IndexFile, sync};

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn write(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

fn sha1(content: &[u8]) -> String {
    use sha1::{Digest, Sha1};
    hex::encode(Sha1::digest(content))
}

struct Setup {
    _temp: tempfile::TempDir,
    repo: PathBuf,
    game: PathBuf,
    server: PathBuf,
    storage: PathBuf,
    ctx: Ctx,
}

impl Setup {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let repo = root.join("repo");
        let prism = root.join("prism");
        let game = prism.join("minecraft");
        let server = root.join("server");
        let storage = root.join("site/data");
        std::fs::create_dir_all(&storage).unwrap();
        std::fs::create_dir_all(&server).unwrap();
        let storage_url = url::Url::from_directory_path(&storage).unwrap();
        write(
            &repo.join("potato.toml"),
            &format!(
                r#"id = "test"
minecraft = "1.21.1"
loader = "fabric"

[publish]
base_url = "https://mc.example.com/data"
target = "{storage_url}"

[[client.optional_sets]]
id = "extras"
display_name = "Extras"
mods = ["extra"]

[[paths]]
path = "config"
type = "directory"
skip_if_dir_exists = true

[[paths]]
path = "kubejs"
type = "directory"

[servers.main]
exclude = ["config/excluded.toml"]
"#
            ),
        );
        write(&repo.join(".gitignore"), ".potato/\n");
        write(
            &repo.join(".potato/local.toml"),
            &format!(
                "[targets.prism]\nkind = \"prism\"\npath = {}\n[targets.main]\nkind = \"server\"\npath = {}\n",
                toml_edit::value(prism.to_string_lossy().as_ref()),
                toml_edit::value(server.to_string_lossy().as_ref())
            ),
        );
        git(&repo, &["init", "-q", "-b", "master"]);
        git(&repo, &["config", "user.name", "t"]);
        git(&repo, &["config", "user.email", "t@t"]);
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-qm", "init"]);

        write(
            &prism.join("mmc-pack.json"),
            r#"{"components":[{"uid":"net.minecraft","version":"1.21.4"},{"uid":"net.fabricmc.fabric-loader","version":"0.16.9"}]}"#,
        );
        write(&game.join("config/a.toml"), "a = 1\n");
        write(&game.join("config/excluded.toml"), "x = 1\n");
        write(&game.join("kubejs/s.js"), "one\n");
        write(&game.join("options.txt"), "fov:90\n");
        let ctx = Ctx::with_cache(&repo, root.join("cache"));
        Self {
            _temp: temp,
            repo,
            game,
            server,
            storage,
            ctx,
        }
    }

    /// Puts a jar in Prism with an index file, and in the download cache.
    async fn prism_mod(&self, name: &str, filename: &str, content: &[u8], index: Option<&str>) {
        let path = self.game.join("mods").join(filename);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
        self.ctx.files.insert(&path, &sha1(content)).await.unwrap();
        if let Some(index) = index {
            write(
                &self.game.join(format!("mods/.index/{name}.pw.toml")),
                index,
            );
        }
    }

    fn commit(&self, message: &str) {
        git(&self.repo, &["add", "-A"]);
        git(&self.repo, &["commit", "-qm", message]);
    }

    fn repo_index(&self, name: &str) -> IndexFile {
        IndexFile::parse(&read(
            &self.repo.join(format!("mods/.index/{name}.pw.toml")),
        ))
        .unwrap()
    }
}

const MODRINTH_INDEX: &str = r#"filename = "sodium.jar"
name = "Sodium"
side = "client"

[download]
hash = "ffff"
hash-format = "sha512"
url = "https://cdn.modrinth.com/data/x/sodium.jar"
"#;

const CURSEFORGE_INDEX: &str = r#"filename = "jei.jar"
name = "JEI"
side = "both"

[download]
hash = "d41d8cd98f00b204e9800998ecf8427e"
hash-format = "md5"
mode = "metadata:curseforge"
"#;

const EXTRA_INDEX: &str = r#"filename = "extra.jar"
name = "Extra"

[download]
hash-format = "sha1"
hash = "0"
url = "https://example.com/extra.jar"
"#;

#[tokio::test]
async fn pull_push_and_status() {
    let mut s = Setup::new();
    s.prism_mod("sodium", "sodium.jar", b"sodium", Some(MODRINTH_INDEX))
        .await;
    s.prism_mod("jei", "jei.jar.disabled", b"jei", Some(CURSEFORGE_INDEX))
        .await;

    // First pull takes Prism as it is, uploads the CurseForge jar and reads versions.
    sync::pull(&mut s.ctx, "prism", &[], false).await.unwrap();
    s.commit("pull");
    assert_eq!(read(&s.repo.join("config/a.toml")), "a = 1\n");
    assert!(!s.repo.join("options.txt").exists());
    assert!(!s.repo.join("mods/sodium.jar").exists());
    let sodium = s.repo_index("sodium");
    assert_eq!(sodium.sha1(), Some(sha1(b"sodium").as_str()));
    assert_eq!(
        sodium.url(),
        Some("https://cdn.modrinth.com/data/x/sodium.jar")
    );
    let jei_sha1 = sha1(b"jei");
    let jei = s.repo_index("jei");
    assert_eq!(
        jei.url().unwrap(),
        format!(
            "https://mc.example.com/data/objects/{}/{jei_sha1}",
            &jei_sha1[..2]
        )
    );
    assert!(
        s.storage
            .join(format!("objects/{}/{jei_sha1}", &jei_sha1[..2]))
            .exists()
    );
    let config = s.ctx.config().unwrap();
    assert_eq!(config.minecraft, "1.21.4");
    assert_eq!(config.loader_version.as_deref(), Some("0.16.9"));

    // Prism's own index files stay untouched; nothing differs by jar.
    assert_eq!(
        read(&s.game.join("mods/.index/sodium.pw.toml")),
        MODRINTH_INDEX
    );
    sync::pull(&mut s.ctx, "prism", &[], false).await.unwrap();
    assert!(s.ctx.git.is_clean().unwrap());

    // A side override in the repo survives pulls while the jar stays the same.
    let mut overridden = s.repo_index("jei");
    overridden.set_side(crate::index::Side::Client);
    std::fs::write(
        s.repo.join("mods/.index/jei.pw.toml"),
        overridden.to_bytes(),
    )
    .unwrap();
    write(&s.repo.join("config/a.toml"), "a = 2\n");
    s.commit("repo edits");
    write(&s.game.join("kubejs/s.js"), "two\n");
    sync::pull(&mut s.ctx, "prism", &[], false).await.unwrap();
    s.commit("merge");
    let parents = git(&s.repo, &["rev-list", "--parents", "-n", "1", "HEAD"]);
    assert_eq!(
        parents.split_whitespace().count(),
        2,
        "pulls make one-parent commits"
    );
    assert_eq!(read(&s.repo.join("kubejs/s.js")), "two\n");
    assert_eq!(read(&s.repo.join("config/a.toml")), "a = 2\n");
    assert_eq!(
        s.repo_index("jei").side().unwrap(),
        crate::index::Side::Client
    );

    // Pushing writes the repo's config, keeps Prism's disabled state.
    sync::push(&mut s.ctx, "prism", false).await.unwrap();
    assert_eq!(read(&s.game.join("config/a.toml")), "a = 2\n");
    assert!(s.game.join("mods/jei.jar.disabled").exists());
    assert!(!s.game.join("mods/jei.jar").exists());

    // Conflicting edits become git conflicts.
    write(&s.repo.join("config/a.toml"), "a = 3\n");
    s.commit("repo a=3");
    write(&s.game.join("config/a.toml"), "a = 4\n");
    sync::pull(&mut s.ctx, "prism", &[], false).await.unwrap();
    assert!(git(&s.repo, &["diff", "--name-only", "--diff-filter=U"]).contains("config/a.toml"));
    assert!(!s.repo.join(".git/CHERRY_PICK_HEAD").exists());
    write(&s.repo.join("config/a.toml"), "a = 4\n");
    s.commit("resolve");
    assert_eq!(git(&s.repo, &["log", "-1", "--format=%an"]).trim(), "t");

    // The same edit on both sides pulls cleanly to nothing.
    write(&s.repo.join("config/a.toml"), "a = 5\n");
    s.commit("repo a=5");
    write(&s.game.join("config/a.toml"), "a = 5\n");
    sync::pull(&mut s.ctx, "prism", &[], false).await.unwrap();
    assert!(s.ctx.git.is_clean().unwrap());

    // Removing a mod in the repo removes its jar and index file from Prism.
    std::fs::remove_file(s.repo.join("mods/.index/jei.pw.toml")).unwrap();
    // A new mod in an off-by-default optional set arrives disabled.
    let extra_sha1 = sha1(b"extra");
    write(
        &s.repo.join("mods/.index/extra.pw.toml"),
        &EXTRA_INDEX.replace("hash = \"0\"", &format!("hash = \"{extra_sha1}\"")),
    );
    let extra_jar = s.repo.join("../extra.jar");
    std::fs::write(&extra_jar, b"extra").unwrap();
    s.ctx.files.insert(&extra_jar, &extra_sha1).await.unwrap();
    s.commit("remove jei, add extra");
    sync::push(&mut s.ctx, "prism", false).await.unwrap();
    assert!(!s.game.join("mods/jei.jar.disabled").exists());
    assert!(!s.game.join("mods/.index/jei.pw.toml").exists());
    assert!(s.game.join("mods/extra.jar.disabled").exists());
    assert!(s.game.join("mods/.index/extra.pw.toml").exists());

    // Unsynced Prism changes block push.
    write(&s.game.join("config/new.toml"), "n = 1\n");
    let err = sync::push(&mut s.ctx, "prism", false).await.unwrap_err();
    assert!(err.to_string().contains("pull"), "{err}");
    sync::status(&mut s.ctx).await.unwrap();
}

#[tokio::test]
async fn os_variants_are_left_alone() {
    let mut s = Setup::new();
    let other = if crate::index::current_os() == "linux" {
        "windows"
    } else {
        "linux"
    };
    write(
        &s.repo.join("mods/.index/voxy.pw.toml"),
        &format!(
            "filename = \"voxy.jar\"\nname = \"Voxy\"\nos = [\"{other}\"]\n\n[download]\nurl = \"https://x/voxy-other.jar\"\nhash-format = \"sha1\"\nhash = \"{}\"\n",
            sha1(b"other")
        ),
    );
    s.commit("other os voxy");
    s.prism_mod(
        "voxy",
        "voxy.jar",
        b"mine",
        Some(
            &EXTRA_INDEX
                .replace("extra.jar", "voxy.jar")
                .replace("hash = \"0\"", &format!("hash = \"{}\"", sha1(b"mine"))),
        ),
    )
    .await;
    sync::pull(&mut s.ctx, "prism", &[], false).await.unwrap();
    s.commit("pull");
    assert_eq!(s.repo_index("voxy").os().unwrap(), vec![other]);
    let mine = s.repo_index(&format!("voxy-{}", crate::index::current_os()));
    assert_eq!(mine.sha1(), Some(sha1(b"mine").as_str()));
}

#[tokio::test]
async fn packs_need_a_path() {
    let mut s = Setup::new();
    write(&s.game.join("resourcepacks/pack.zip"), "zip");
    write(
        &s.game.join("resourcepacks/.index/pack.pw.toml"),
        "filename = \"pack.zip\"\nname = \"Pack\"\n\n[download]\nurl = \"https://cdn.modrinth.com/data/p/pack.zip\"\nhash-format = \"sha1\"\nhash = \"x\"\n",
    );
    sync::pull(&mut s.ctx, "prism", &[], false).await.unwrap();
    s.commit("pull");
    assert!(!s.repo.join("resourcepacks").exists());

    let config = read(&s.repo.join("potato.toml"));
    write(
        &s.repo.join("potato.toml"),
        &format!("{config}\n[[paths]]\npath = \"resourcepacks\"\ntype = \"directory\"\n"),
    );
    s.commit("sync resourcepacks");
    sync::pull(&mut s.ctx, "prism", &[], false).await.unwrap();
    assert!(s.repo.join("resourcepacks/.index/pack.pw.toml").exists());
}

#[tokio::test]
async fn uncommitted_changes() {
    let mut s = Setup::new();
    sync::pull(&mut s.ctx, "prism", &[], false).await.unwrap();
    s.commit("pull");

    // Files outside the pack never block a push.
    write(&s.repo.join("README.md"), "notes\n");
    sync::push(&mut s.ctx, "prism", false).await.unwrap();

    // Uncommitted pack files do, since push would leave them out.
    write(&s.repo.join("config/a.toml"), "a = 7\n");
    let err = sync::push(&mut s.ctx, "prism", false).await.unwrap_err();
    assert!(err.to_string().contains("config/a.toml"), "{err}");
    git(&s.repo, &["checkout", "-q", "--", "config/a.toml"]);

    // Pulls stack up without committing in between.
    write(&s.game.join("kubejs/s.js"), "one\n");
    sync::pull(&mut s.ctx, "prism", &[], false).await.unwrap();
    write(&s.game.join("config/a.toml"), "a = 9\n");
    sync::pull(&mut s.ctx, "prism", &[], false).await.unwrap();
    assert_eq!(read(&s.repo.join("kubejs/s.js")), "one\n");
    assert_eq!(read(&s.repo.join("config/a.toml")), "a = 9\n");
    let err = sync::push(&mut s.ctx, "prism", false).await.unwrap_err();
    assert!(err.to_string().contains("won't be pushed"), "{err}");
}

#[tokio::test]
async fn discarded_pull_comes_back_with_force() {
    let mut s = Setup::new();
    s.prism_mod("sodium", "sodium.jar", b"sodium", Some(MODRINTH_INDEX))
        .await;
    sync::pull(&mut s.ctx, "prism", &[], false).await.unwrap();
    assert!(s.repo.join(".potato/synced/prism").exists());
    assert!(!s.game.join("../.potato").exists());
    git(&s.repo, &["reset", "-q", "--hard"]);

    // The record still matches Prism, so only --force brings the mod back.
    sync::pull(&mut s.ctx, "prism", &[], false).await.unwrap();
    assert!(!s.repo.join("mods/.index/sodium.pw.toml").exists());
    s.commit("versions");
    sync::pull(&mut s.ctx, "prism", &[], true).await.unwrap();
    assert!(s.repo.join("mods/.index/sodium.pw.toml").exists());
}

#[tokio::test]
async fn server_sync() {
    let mut s = Setup::new();
    s.prism_mod("sodium", "sodium.jar", b"sodium", Some(MODRINTH_INDEX))
        .await;
    s.prism_mod("jei", "jei.jar", b"jei", Some(CURSEFORGE_INDEX))
        .await;
    sync::pull(&mut s.ctx, "prism", &[], false).await.unwrap();
    s.commit("pull");

    // A never-synced server can be pulled; it has nothing the repo lacks.
    sync::pull(&mut s.ctx, "main", &[], false).await.unwrap();
    assert!(s.repo.join("config/a.toml").exists());
    let err = sync::push(&mut s.ctx, "main", false).await.unwrap_err();
    assert!(err.to_string().contains("--force"), "{err}");

    write(&s.server.join("mods/server-only.jar"), "mine");
    write(&s.server.join("config/server-only.toml"), "s = 1\n");
    sync::push(&mut s.ctx, "main", true).await.unwrap();
    assert_eq!(read(&s.server.join("config/a.toml")), "a = 1\n");
    assert!(!s.server.join("config/excluded.toml").exists());
    assert!(s.server.join("mods/jei.jar").exists());
    assert!(
        !s.server.join("mods/sodium.jar").exists(),
        "client-only mod"
    );
    assert!(!s.server.join("mods/.index").exists());
    assert!(s.server.join("mods/server-only.jar").exists());

    // Server pulls only take changes to files the repo already has.
    write(&s.server.join("config/a.toml"), "a = 5\n");
    write(&s.server.join("config/excluded.toml"), "mine\n");
    sync::pull(&mut s.ctx, "main", &[], false).await.unwrap();
    s.commit("pull server");
    assert_eq!(read(&s.repo.join("config/a.toml")), "a = 5\n");
    assert!(!s.repo.join("config/server-only.toml").exists());
    assert_eq!(read(&s.repo.join("config/excluded.toml")), "x = 1\n");
    sync::pull(
        &mut s.ctx,
        "main",
        &["config/server-only.toml".into()],
        false,
    )
    .await
    .unwrap();
    s.commit("add server file");
    assert!(s.repo.join("config/server-only.toml").exists());

    // Removing a mod deletes only the jar potato put there.
    std::fs::remove_file(s.repo.join("mods/.index/jei.pw.toml")).unwrap();
    s.commit("remove jei");
    sync::push(&mut s.ctx, "main", false).await.unwrap();
    assert!(!s.server.join("mods/jei.jar").exists());
    assert!(s.server.join("mods/server-only.jar").exists());
    assert!(s.server.join("config/server-only.toml").exists());

    write(&s.server.join("config/a.toml"), "a = 6\n");
    let err = sync::push(&mut s.ctx, "main", false).await.unwrap_err();
    assert!(err.to_string().contains("changed"), "{err}");
}

async fn publish_real_instance(loader: &str, loader_version: &str) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let repo = root.join(format!("{loader}-pack"));
    let storage = root.join("site/data");
    std::fs::create_dir_all(&storage).unwrap();
    let storage_url = url::Url::from_directory_path(&storage).unwrap();
    write(
        &repo.join("potato.toml"),
        &format!(
            r#"id = "{loader}-test"
minecraft = "1.21.1"
loader = "{loader}"
loader_version = "{loader_version}"
order = 1

[publish]
base_url = "https://mc.example.com/data"
target = "{storage_url}"

[[paths]]
path = "config"
type = "directory"
"#
        ),
    );
    write(&repo.join("config/my file.toml"), "a = 1\n");
    write(&repo.join(".gitignore"), ".potato/\n");
    git(&repo, &["init", "-q", "-b", "master"]);
    git(&repo, &["config", "user.name", "t"]);
    git(&repo, &["config", "user.email", "t@t"]);
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "init"]);
    let mut ctx = Ctx::with_cache(&repo, crate::config::cache_dir().unwrap());
    Box::pin(crate::publish::publish(&mut ctx, false))
        .await
        .unwrap();

    let manifest: instance::manifest::InstanceManifest =
        serde_json::from_slice(&std::fs::read(storage.join("instance_manifest.json")).unwrap())
            .unwrap();
    let entry = &manifest.instances[0];
    let meta_path = storage.join(format!("instances/{loader}-test/meta.json"));
    let meta: instance::instance_metadata::InstanceMetadata =
        serde_json::from_slice(&std::fs::read(&meta_path).unwrap()).unwrap();
    assert_eq!(entry.sha1, utils::hash_struct(&meta).unwrap());
    assert_eq!(
        utils::files::hash_file(&meta_path).await.unwrap(),
        entry.sha1
    );
    let rule_url = serde_json::to_string(&meta.content_rules).unwrap();
    assert!(
        rule_url.contains("https://mc.example.com/data/objects/"),
        "{rule_url}"
    );
    let config_sha1 = sha1(b"a = 1\n");
    assert!(
        storage
            .join(format!("objects/{}/{config_sha1}", &config_sha1[..2]))
            .exists()
    );

    // Republishing the same commit works; the history keeps the old record.
    Box::pin(crate::publish::publish(&mut ctx, false))
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_dir(storage.join(format!("history/{loader}-test")))
            .unwrap()
            .count(),
        1
    );

    // Another repo can't take over the same id.
    let other = root.join("other");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::copy(repo.join("potato.toml"), other.join("potato.toml")).unwrap();
    std::fs::copy(repo.join(".gitignore"), other.join(".gitignore")).unwrap();
    git(&other, &["init", "-q", "-b", "master"]);
    git(&other, &["config", "user.name", "t"]);
    git(&other, &["config", "user.email", "t@t"]);
    git(&other, &["add", "-A"]);
    git(&other, &["commit", "-qm", "other"]);
    let mut other_ctx = Ctx::with_cache(&other, crate::config::cache_dir().unwrap());
    let err = Box::pin(crate::publish::publish(&mut other_ctx, false))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("another repo"), "{err}");
}

#[test]
#[ignore = "uses the network and runs a real build"]
fn publishes_fabric_instance() {
    crate::block_on_big_stack(publish_real_instance("fabric", "0.16.9"));
}

#[test]
#[ignore = "uses the network and runs the NeoForge installer"]
fn publishes_neoforge_instance() {
    crate::block_on_big_stack(publish_real_instance("neoforge", "21.1.77"));
}
