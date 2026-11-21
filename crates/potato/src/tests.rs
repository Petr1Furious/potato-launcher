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
    sync::pull(&mut s.ctx, "prism", false).await.unwrap();
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
    // The CurseForge jar has no URL, so it's committed, and its index file only describes it.
    assert_eq!(read(&s.repo.join("mods/jei.jar")), "jei");
    assert_eq!(s.repo_index("jei").url(), None);
    assert!(!s.storage.join("objects").exists());
    let config = s.ctx.config().unwrap();
    assert_eq!(config.minecraft, "1.21.4");
    assert_eq!(config.loader_version.as_deref(), Some("0.16.9"));

    // Prism's own index files stay untouched; nothing differs by jar.
    assert_eq!(
        read(&s.game.join("mods/.index/sodium.pw.toml")),
        MODRINTH_INDEX
    );
    sync::pull(&mut s.ctx, "prism", false).await.unwrap();
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
    sync::pull(&mut s.ctx, "prism", false).await.unwrap();
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
    sync::pull(&mut s.ctx, "prism", false).await.unwrap();
    assert!(git(&s.repo, &["diff", "--name-only", "--diff-filter=U"]).contains("config/a.toml"));
    assert!(!s.repo.join(".git/CHERRY_PICK_HEAD").exists());
    write(&s.repo.join("config/a.toml"), "a = 4\n");
    s.commit("resolve");
    assert_eq!(git(&s.repo, &["log", "-1", "--format=%an"]).trim(), "t");

    // The same edit on both sides pulls cleanly to nothing.
    write(&s.repo.join("config/a.toml"), "a = 5\n");
    s.commit("repo a=5");
    write(&s.game.join("config/a.toml"), "a = 5\n");
    sync::pull(&mut s.ctx, "prism", false).await.unwrap();
    assert!(s.ctx.git.is_clean().unwrap());

    // Removing a mod in the repo removes its jar and index file from Prism.
    std::fs::remove_file(s.repo.join("mods/.index/jei.pw.toml")).unwrap();
    std::fs::remove_file(s.repo.join("mods/jei.jar")).unwrap();
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
    sync::pull(&mut s.ctx, "prism", false).await.unwrap();
    s.commit("pull");
    assert_eq!(s.repo_index("voxy").os().unwrap(), vec![other]);
    let mine = s.repo_index(&format!("voxy-{}", crate::index::current_os()));
    assert_eq!(mine.sha1(), Some(sha1(b"mine").as_str()));
}

fn voxy_index(filename: &str) -> String {
    format!(
        "filename = \"{filename}\"\nname = \"Voxy\"\nside = \"client\"\n\n[download]\nhash = \"ffff\"\nhash-format = \"sha512\"\nurl = \"https://cdn.modrinth.com/data/v/{filename}\"\n\n[update.modrinth]\nmod-id = \"VOXY\"\nversion = \"1\"\n"
    )
}

#[tokio::test]
async fn renamed_jar_updates_its_os_variant() {
    let mut s = Setup::new();
    let os = crate::index::current_os();
    s.prism_mod(
        "voxy",
        "voxy-1.jar",
        b"voxy 1",
        Some(&voxy_index("voxy-1.jar")),
    )
    .await;
    sync::pull(&mut s.ctx, "prism", false).await.unwrap();
    crate::commands::mod_set(&s.ctx, "voxy", &[os.to_string()], None).unwrap();
    let mut other = IndexFile::new(
        "Voxy",
        "voxy-other.jar",
        "https://cdn.modrinth.com/data/v/voxy-other.jar",
        "ffff",
    );
    let other_os = if os == "windows" { "linux" } else { "windows" };
    other.set_os(&[other_os]);
    std::fs::write(
        s.repo.join("mods/.index/voxy-other.pw.toml"),
        other.to_bytes(),
    )
    .unwrap();
    s.commit("two voxies");

    // Prism replaces the jar under a new name but keeps its own index name.
    std::fs::remove_file(s.game.join("mods/voxy-1.jar")).unwrap();
    s.prism_mod(
        "voxy",
        "voxy-2.jar",
        b"voxy 2",
        Some(&voxy_index("voxy-2.jar")),
    )
    .await;
    sync::pull(&mut s.ctx, "prism", false).await.unwrap();
    let updated = s.repo_index("voxy");
    assert_eq!(updated.filename(), Some("voxy-2.jar"));
    assert_eq!(updated.os().unwrap(), vec![os]);
    assert_eq!(s.repo_index("voxy-other").os().unwrap(), vec![other_os]);
    assert!(
        !s.repo
            .join(format!("mods/.index/voxy-{os}.pw.toml"))
            .exists()
    );
}

#[tokio::test]
async fn packs_need_a_path() {
    let mut s = Setup::new();
    write(&s.game.join("resourcepacks/pack.zip"), "zip");
    write(
        &s.game.join("resourcepacks/.index/pack.pw.toml"),
        "filename = \"pack.zip\"\nname = \"Pack\"\n\n[download]\nurl = \"https://cdn.modrinth.com/data/p/pack.zip\"\nhash-format = \"sha1\"\nhash = \"x\"\n",
    );
    sync::pull(&mut s.ctx, "prism", false).await.unwrap();
    s.commit("pull");
    assert!(!s.repo.join("resourcepacks").exists());

    let config = read(&s.repo.join("potato.toml"));
    write(
        &s.repo.join("potato.toml"),
        &format!("{config}\n[[paths]]\npath = \"resourcepacks\"\ntype = \"directory\"\n"),
    );
    s.commit("sync resourcepacks");
    sync::pull(&mut s.ctx, "prism", false).await.unwrap();
    assert!(s.repo.join("resourcepacks/.index/pack.pw.toml").exists());
}

#[tokio::test]
async fn forced_push_makes_prism_match() {
    let mut s = Setup::new();
    s.prism_mod("sodium", "sodium.jar", b"sodium", Some(MODRINTH_INDEX))
        .await;
    s.prism_mod("jei", "jei.jar.disabled", b"jei", Some(CURSEFORGE_INDEX))
        .await;
    sync::pull(&mut s.ctx, "prism", false).await.unwrap();
    s.commit("pull");

    write(&s.game.join("config/a.toml"), "a = 8\n");
    write(&s.game.join("config/extra.toml"), "x\n");
    write(&s.game.join("mods/extra.jar"), "extra");
    write(&s.game.join("mods/old.jar.disabled"), "old");
    write(
        &s.game.join("mods/.index/extra.pw.toml"),
        &MODRINTH_INDEX.replace("sodium.jar", "extra.jar"),
    );
    let err = sync::push(&mut s.ctx, "prism", false).await.unwrap_err();
    assert!(err.to_string().contains("haven't committed"), "{err}");

    sync::push(&mut s.ctx, "prism", true).await.unwrap();
    assert_eq!(read(&s.game.join("config/a.toml")), "a = 1\n");
    assert!(!s.game.join("config/extra.toml").exists());
    assert!(!s.game.join("mods/extra.jar").exists());
    assert!(!s.game.join("mods/old.jar.disabled").exists());
    assert!(!s.game.join("mods/.index/extra.pw.toml").exists());
    assert!(s.game.join("mods/sodium.jar").exists());
    assert!(
        s.game.join("mods/jei.jar.disabled").exists(),
        "keeps the disabled state"
    );
}

#[tokio::test]
async fn uncommitted_changes() {
    let mut s = Setup::new();
    sync::pull(&mut s.ctx, "prism", false).await.unwrap();
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
    sync::pull(&mut s.ctx, "prism", false).await.unwrap();
    write(&s.game.join("config/a.toml"), "a = 9\n");
    sync::pull(&mut s.ctx, "prism", false).await.unwrap();
    assert_eq!(read(&s.repo.join("kubejs/s.js")), "one\n");
    assert_eq!(read(&s.repo.join("config/a.toml")), "a = 9\n");
    let err = sync::push(&mut s.ctx, "prism", false).await.unwrap_err();
    assert!(err.to_string().contains("uncommitted changes"), "{err}");
}

#[tokio::test]
async fn discarded_pull_comes_back() {
    let mut s = Setup::new();
    sync::pull(&mut s.ctx, "prism", false).await.unwrap();
    s.commit("pull");
    sync::push(&mut s.ctx, "prism", false).await.unwrap();
    assert!(s.repo.join(".potato/synced/prism").exists());

    s.prism_mod("sodium", "sodium.jar", b"sodium", Some(MODRINTH_INDEX))
        .await;
    sync::pull(&mut s.ctx, "prism", false).await.unwrap();
    git(&s.repo, &["reset", "-q", "--hard"]);
    sync::pull(&mut s.ctx, "prism", false).await.unwrap();
    assert!(s.repo.join("mods/.index/sodium.pw.toml").exists());
    git(&s.repo, &["reset", "-q", "--hard"]);

    // Pushing would drop the mod from Prism.
    let err = sync::push(&mut s.ctx, "prism", false).await.unwrap_err();
    assert!(err.to_string().contains("pull"), "{err}");
}

#[tokio::test]
async fn committed_changes_are_not_pulled_again() {
    let mut s = Setup::new();
    sync::pull(&mut s.ctx, "prism", false).await.unwrap();
    s.commit("pull");
    sync::push(&mut s.ctx, "prism", false).await.unwrap();

    write(&s.game.join("config/a.toml"), "a = 2\n");
    sync::pull(&mut s.ctx, "prism", false).await.unwrap();
    s.commit("take a = 2");
    write(&s.repo.join("config/a.toml"), "a = 3\n");
    s.commit("a = 3");

    // Prism still has the a = 2 you already took; only its new change comes in.
    write(&s.game.join("kubejs/s.js"), "new\n");
    sync::pull(&mut s.ctx, "prism", false).await.unwrap();
    assert!(git(&s.repo, &["diff", "--name-only", "--diff-filter=U"]).is_empty());
    assert_eq!(read(&s.repo.join("config/a.toml")), "a = 3\n");
    assert_eq!(read(&s.repo.join("kubejs/s.js")), "new\n");
    s.commit("take s.js");

    sync::push(&mut s.ctx, "prism", false).await.unwrap();
    assert_eq!(read(&s.game.join("config/a.toml")), "a = 3\n");
}

#[tokio::test]
async fn server_sync() {
    let mut s = Setup::new();
    s.prism_mod("sodium", "sodium.jar", b"sodium", Some(MODRINTH_INDEX))
        .await;
    s.prism_mod("jei", "jei.jar", b"jei", Some(CURSEFORGE_INDEX))
        .await;
    sync::pull(&mut s.ctx, "prism", false).await.unwrap();
    s.commit("pull");

    // Servers only get pushes; a first push needs --force only to overwrite files.
    let err = sync::pull(&mut s.ctx, "main", false).await.unwrap_err();
    assert!(
        err.to_string().contains("can't pull from a server"),
        "{err}"
    );
    write(&s.server.join("config/a.toml"), "mine\n");
    let err = sync::push(&mut s.ctx, "main", false).await.unwrap_err();
    assert!(err.to_string().contains("config/a.toml"), "{err}");

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

    // An edit made on the server blocks the push until the repo has it too.
    write(&s.server.join("config/a.toml"), "a = 5\n");
    let err = sync::push(&mut s.ctx, "main", false).await.unwrap_err();
    assert!(err.to_string().contains("config/a.toml"), "{err}");
    write(&s.repo.join("config/a.toml"), "a = 5\n");
    s.commit("take the server's edit");
    sync::push(&mut s.ctx, "main", false).await.unwrap();

    // Removing a mod deletes only the jar potato put there.
    std::fs::remove_file(s.repo.join("mods/.index/jei.pw.toml")).unwrap();
    std::fs::remove_file(s.repo.join("mods/jei.jar")).unwrap();
    s.commit("remove jei");
    sync::push(&mut s.ctx, "main", false).await.unwrap();
    assert!(!s.server.join("mods/jei.jar").exists());
    assert!(s.server.join("mods/server-only.jar").exists());
    assert!(s.server.join("config/server-only.toml").exists());
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

[[paths]]
path = "resourcepacks"
type = "directory"
"#
        ),
    );
    write(&repo.join("config/my file.toml"), "a = 1\n");
    write(&repo.join("resourcepacks/pack.zip"), "committed pack");
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
    assert_eq!(entry.minecraft_version.as_deref(), Some("1.21.1"));
    let expected_loader = if loader == "fabric" {
        "Fabric"
    } else {
        "NeoForge"
    };
    assert_eq!(
        entry.loader.as_deref(),
        Some(expected_loader),
        "{:?}",
        meta.versions.iter().map(|v| &v.id).collect::<Vec<_>>()
    );
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
    // A committed pack is uploaded like any file only we host.
    let pack_sha1 = sha1(b"committed pack");
    assert!(
        storage
            .join(format!("objects/{}/{pack_sha1}", &pack_sha1[..2]))
            .exists()
    );
    assert!(rule_url.contains(&pack_sha1), "{rule_url}");

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

#[tokio::test]
async fn committed_jars_need_no_pull() {
    let mut s = Setup::new();
    sync::pull(&mut s.ctx, "prism", false).await.unwrap();
    s.commit("pull");
    sync::push(&mut s.ctx, "prism", false).await.unwrap();

    // A jar built straight into the repo goes out with the next push.
    write(&s.repo.join("mods/mymod.jar"), "v1");
    s.commit("mymod");
    sync::push(&mut s.ctx, "prism", false).await.unwrap();
    assert_eq!(read(&s.game.join("mods/mymod.jar")), "v1");
    assert!(!s.game.join("mods/.index/mymod.pw.toml").exists());

    write(&s.repo.join("mods/mymod.jar"), "v2");
    let err = sync::push(&mut s.ctx, "prism", false).await.unwrap_err();
    assert!(err.to_string().contains("mods/mymod.jar"), "{err}");
    s.commit("mymod v2");
    sync::push(&mut s.ctx, "prism", false).await.unwrap();
    assert_eq!(read(&s.game.join("mods/mymod.jar")), "v2");

    // A build dropped into Prism instead comes back as the committed file.
    write(&s.game.join("mods/mymod.jar"), "v3");
    sync::pull(&mut s.ctx, "prism", false).await.unwrap();
    assert_eq!(read(&s.repo.join("mods/mymod.jar")), "v3");
    s.commit("mymod v3");

    // An index file without a URL sets its side; servers then don't get it.
    crate::commands::mod_set(&s.ctx, "mymod", &[], Some("client")).unwrap();
    let index = s.repo_index("mymod");
    assert_eq!(index.url(), None);
    assert_eq!(index.filename(), Some("mymod.jar"));
    s.commit("mymod is client-only");
    sync::push(&mut s.ctx, "main", true).await.unwrap();
    assert!(!s.server.join("mods/mymod.jar").exists());
}

#[tokio::test]
async fn committed_jar_and_download_url_conflict() {
    let mut s = Setup::new();
    write(&s.repo.join("mods/sodium.jar"), "sodium");
    write(&s.repo.join("mods/.index/sodium.pw.toml"), MODRINTH_INDEX);
    s.commit("both");
    let err = sync::push(&mut s.ctx, "prism", true).await.unwrap_err();
    assert!(err.to_string().contains("remove one"), "{err}");
}

#[tokio::test]
async fn enabled_and_disabled_copies_conflict() {
    let mut s = Setup::new();
    s.prism_mod("jei", "jei.jar.disabled", b"old", None).await;
    sync::pull(&mut s.ctx, "prism", false).await.unwrap();
    s.commit("pull");

    s.prism_mod("jei", "jei.jar", b"new", None).await;
    let err = sync::pull(&mut s.ctx, "prism", false).await.unwrap_err();
    assert!(
        err.to_string()
            .contains("mods/jei.jar is both enabled and disabled"),
        "{err}"
    );
    let err = sync::status(&mut s.ctx).await.unwrap_err();
    assert!(err.to_string().contains("remove one"), "{err}");
}

#[tokio::test]
async fn enabled_jars_with_one_mod_id_conflict() {
    let mut s = Setup::new();
    s.prism_mod(
        "voxy-old",
        "voxy-old.jar.disabled",
        &fabric_jar("voxy", "1.0").await,
        None,
    )
    .await;
    s.prism_mod("voxy", "voxy.jar", &fabric_jar("voxy", "2.0").await, None)
        .await;
    sync::pull(&mut s.ctx, "prism", false).await.unwrap();
    s.commit("pull");

    s.prism_mod(
        "voxy-fork",
        "voxy-fork.jar",
        &fabric_jar("voxy", "3.0").await,
        None,
    )
    .await;
    let err = sync::pull(&mut s.ctx, "prism", false).await.unwrap_err();
    assert!(
        err.to_string()
            .contains("mods/voxy-fork.jar and mods/voxy.jar are both mod \"voxy\""),
        "{err}"
    );
}

#[tokio::test]
async fn committed_jars_work_with_git_lfs() {
    let has_lfs = Command::new("git")
        .args(["lfs", "version"])
        .output()
        .is_ok_and(|output| output.status.success());
    if !has_lfs {
        return;
    }
    let mut s = Setup::new();
    git(&s.repo, &["lfs", "install", "--local"]);
    write(
        &s.repo.join(".gitattributes"),
        "mods/*.jar filter=lfs diff=lfs merge=lfs -text\n",
    );
    s.commit("lfs");
    s.prism_mod("jei", "jei.jar", b"jei", Some(CURSEFORGE_INDEX))
        .await;
    sync::pull(&mut s.ctx, "prism", false).await.unwrap();
    s.commit("pull");
    assert_eq!(read(&s.repo.join("mods/jei.jar")), "jei");
    let stored = git(&s.repo, &["cat-file", "-p", "HEAD:mods/jei.jar"]);
    assert!(stored.starts_with("version https://git-lfs"), "{stored}");

    // Pushing reads the jar back through LFS, not the pointer.
    std::fs::remove_file(s.game.join("mods/jei.jar")).unwrap();
    sync::push(&mut s.ctx, "prism", true).await.unwrap();
    assert_eq!(read(&s.game.join("mods/jei.jar")), "jei");
}

async fn fabric_jar(mod_id: &str, version: &str) -> Vec<u8> {
    use async_zip::{Compression, ZipEntryBuilder, base::write::ZipFileWriter};
    let json = format!(r#"{{"schemaVersion":1,"id":"{mod_id}","version":"{version}"}}"#);
    let mut writer = ZipFileWriter::new(Vec::<u8>::new());
    let entry = ZipEntryBuilder::new("fabric.mod.json".into(), Compression::Stored);
    writer
        .write_entry_whole(entry, json.as_bytes())
        .await
        .unwrap();
    writer.close().await.unwrap()
}

#[tokio::test]
async fn renamed_update_keeps_index_and_disabled_state() {
    let mut s = Setup::new();
    s.prism_mod(
        "sodium",
        "sodium-1.jar.disabled",
        b"sodium 1",
        Some(&MODRINTH_INDEX.replace("sodium.jar", "sodium-1.jar")),
    )
    .await;
    sync::pull(&mut s.ctx, "prism", false).await.unwrap();
    s.commit("pull");

    let new_sha1 = sha1(b"sodium 2");
    let jar = s.repo.join("../sodium-2.jar");
    std::fs::write(&jar, b"sodium 2").unwrap();
    s.ctx.files.insert(&jar, &new_sha1).await.unwrap();
    let update = IndexFile::new(
        "Sodium",
        "sodium-2.jar",
        "https://cdn.modrinth.com/data/x/sodium-2.jar",
        &new_sha1,
    );
    std::fs::write(s.repo.join("mods/.index/sodium.pw.toml"), update.to_bytes()).unwrap();
    s.commit("update sodium");
    sync::push(&mut s.ctx, "prism", false).await.unwrap();

    assert!(s.game.join("mods/sodium-2.jar.disabled").exists());
    assert!(!s.game.join("mods/sodium-1.jar.disabled").exists());
    let index = read(&s.game.join("mods/.index/sodium.pw.toml"));
    assert!(index.contains("sodium-2.jar"), "{index}");
}

#[tokio::test]
async fn same_update_on_both_sides_merges_cleanly() {
    let mut s = Setup::new();
    s.prism_mod(
        "sodium",
        "sodium-1.jar",
        b"sodium 1",
        Some(&MODRINTH_INDEX.replace("sodium.jar", "sodium-1.jar")),
    )
    .await;
    sync::pull(&mut s.ctx, "prism", false).await.unwrap();
    s.commit("pull");

    // A collaborator's update, written differently from what Prism writes.
    let update = IndexFile::new(
        "Sodium",
        "sodium-2.jar",
        "https://cdn.modrinth.com/data/x/sodium-2.jar",
        &sha1(b"sodium 2"),
    );
    std::fs::write(s.repo.join("mods/.index/sodium.pw.toml"), update.to_bytes()).unwrap();
    s.commit("update sodium");
    let committed = read(&s.repo.join("mods/.index/sodium.pw.toml"));

    // The same update made in Prism.
    std::fs::remove_file(s.game.join("mods/sodium-1.jar")).unwrap();
    s.prism_mod(
        "sodium",
        "sodium-2.jar",
        b"sodium 2",
        Some(&MODRINTH_INDEX.replace("sodium.jar", "sodium-2.jar")),
    )
    .await;
    sync::pull(&mut s.ctx, "prism", false).await.unwrap();
    assert!(git(&s.repo, &["diff", "--name-only", "--diff-filter=U"]).is_empty());
    assert_eq!(read(&s.repo.join("mods/.index/sodium.pw.toml")), committed);
}

#[tokio::test]
async fn renamed_committed_jar_matches_by_mod_id() {
    let mut s = Setup::new();
    std::fs::create_dir_all(s.repo.join("mods")).unwrap();
    std::fs::write(
        s.repo.join("mods/mymod-1.0.jar"),
        fabric_jar("mymod", "1.0").await,
    )
    .unwrap();
    crate::commands::mod_set(&s.ctx, "mymod-1.0", &[], Some("client")).unwrap();
    s.commit("mymod");
    sync::push(&mut s.ctx, "prism", true).await.unwrap();
    assert!(s.game.join("mods/mymod-1.0.jar").exists());

    // A new build under a new name, with an unrelated Prism index file.
    std::fs::remove_file(s.game.join("mods/mymod-1.0.jar")).unwrap();
    std::fs::write(
        s.game.join("mods/mymod-1.1.jar"),
        fabric_jar("mymod", "1.1").await,
    )
    .unwrap();
    write(
        &s.game.join("mods/.index/something-else.pw.toml"),
        "filename = \"mymod-1.1.jar\"\nname = \"My Mod\"\n",
    );
    sync::pull(&mut s.ctx, "prism", false).await.unwrap();

    assert!(!s.repo.join("mods/mymod-1.0.jar").exists());
    assert!(s.repo.join("mods/mymod-1.1.jar").exists());
    let index = s.repo_index("mymod-1.0");
    assert_eq!(index.filename(), Some("mymod-1.1.jar"));
    assert_eq!(index.side().unwrap(), crate::index::Side::Client);
    assert!(!s.repo.join("mods/.index/something-else.pw.toml").exists());
}

#[tokio::test]
async fn committed_pulls_move_the_record() {
    let mut s = Setup::new();
    s.prism_mod("sodium", "sodium.jar", b"sodium", Some(MODRINTH_INDEX))
        .await;
    sync::pull(&mut s.ctx, "prism", false).await.unwrap();
    s.commit("pull");

    // Without ever pushing, a mod removed in Prism comes in as a deletion.
    std::fs::remove_file(s.game.join("mods/sodium.jar")).unwrap();
    std::fs::remove_file(s.game.join("mods/.index/sodium.pw.toml")).unwrap();
    sync::pull(&mut s.ctx, "prism", false).await.unwrap();
    assert!(s.repo.join(".potato/synced/prism").exists());
    assert!(!s.repo.join("mods/.index/sodium.pw.toml").exists());
}

#[tokio::test]
async fn repo_only_versions_are_not_mistaken_for_pulls() {
    let mut s = Setup::new();
    sync::pull(&mut s.ctx, "prism", false).await.unwrap();
    s.commit("pull");
    sync::push(&mut s.ctx, "prism", false).await.unwrap();

    write(&s.repo.join("config/a.toml"), "a = 80\n");
    s.commit("80");
    write(&s.repo.join("config/a.toml"), "a = 75\n");
    s.commit("75");
    // Prism lands on a value the repo passed through, but it was never pulled.
    write(&s.game.join("config/a.toml"), "a = 80\n");
    sync::pull(&mut s.ctx, "prism", false).await.unwrap();
    assert!(git(&s.repo, &["diff", "--name-only", "--diff-filter=U"]).contains("config/a.toml"));
}

#[tokio::test]
async fn forced_pull_makes_the_repo_match_prism() {
    let mut s = Setup::new();
    sync::pull(&mut s.ctx, "prism", false).await.unwrap();
    s.commit("pull");
    write(&s.repo.join("config/a.toml"), "a = 5\n");
    write(&s.repo.join("config/extra.toml"), "x\n");
    s.commit("repo edits");

    sync::pull(&mut s.ctx, "prism", true).await.unwrap();
    assert_eq!(read(&s.repo.join("config/a.toml")), "a = 1\n");
    assert!(!s.repo.join("config/extra.toml").exists());
    s.commit("take prism");
    sync::push(&mut s.ctx, "prism", false).await.unwrap();
}

#[tokio::test]
async fn differing_files_conflict_without_a_record() {
    let mut s = Setup::new();
    write(&s.repo.join("config/a.toml"), "a = 9\n");
    s.commit("repo config");
    sync::pull(&mut s.ctx, "prism", false).await.unwrap();
    assert!(git(&s.repo, &["diff", "--name-only", "--diff-filter=U"]).contains("config/a.toml"));
}

#[tokio::test]
async fn push_removes_pulled_files_the_repo_dropped() {
    let mut s = Setup::new();
    write(&s.game.join("kubejs/new.js"), "new\n");
    sync::pull(&mut s.ctx, "prism", false).await.unwrap();
    // Committed with another edit, so the record doesn't move.
    write(&s.repo.join("kubejs/other.js"), "other\n");
    s.commit("pull and edit");
    std::fs::remove_file(s.repo.join("kubejs/new.js")).unwrap();
    s.commit("drop new.js");
    sync::push(&mut s.ctx, "prism", false).await.unwrap();
    assert!(!s.game.join("kubejs/new.js").exists());
}
