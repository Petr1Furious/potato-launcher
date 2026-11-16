# Launcher verification

Lets a player check that the files an instance runs are the same files their
original sources publish, without trusting the instance's host.

## Threat model

The host serving instance metadata and files (see `pack-repos.md`) may be
malicious: it may change hashes in the metadata, serve modified jars, add
jars, or change launch arguments.

Trusted:

- the launcher binary itself (self-update is out of scope, see below)
- the player's machine
- HTTPS connections to the original sources listed below

Verification proves that a file is a public file from its original source. It
doesn't prove that the file is safe. A malicious mod published on Modrinth
passes.

## Core rule

The launcher hashes every file on disk itself and asks the file's original
source whether that hash belongs to it. It never uses a hash from the host's
metadata.

The original sources are hard-coded in the launcher. A source named in the
metadata (say, a library URL on some other Maven repo) is never used for
checking.

## What is checked

| Files | Checked against |
|---|---|
| Client jar, Mojang libraries, assets | Mojang's version manifest, version JSON and asset index, fetched by the launcher for the instance's MC version |
| Loader libraries | `.sha1` files next to the artifact on the loader's official Maven repo (Fabric, NeoForge, Forge), by the library's coordinates |
| Forge/NeoForge patched jars | Expected output hashes in the official installer's `install_profile.json` (`PATCHED_SHA` and similar). Unsure: confirm these hashes cover every file the processors produce. |
| Modrinth mods | One `POST /v2/version_files` request with every jar's sha1 |
| CurseForge mods | Unsure, see "Open questions" |
| Java | sha256 of the runtime archive, compared with Azul's API |
| authlib-injector | sha256 from its official release API |

Java is checked through its archive: the launcher records each archive's
sha256 when it extracts it, and verification checks that against Azul. The
extracted files aren't checked individually, because the player's machine is
trusted.

## What is scanned

Verification checks what's actually on disk and on the command line, not only
what the metadata lists:

- every `.jar` anywhere in the instance folder (a content rule can place one
  outside the mod list)
- every classpath entry and every `-javaagent`
- the main class and all JVM and game arguments

### Launch arguments

The launcher builds the arguments from the original Mojang and loader version
JSONs and compares them with the arguments it would actually launch with.
Allowed differences:

- authlib-injector's `-javaagent`, pointing at a verified authlib-injector jar
- `-Xmx` / `-Xms`
- the launcher's own fixed flags

Anything else is a mismatch.

## Results

Each file or argument ends up in one state:

- **Verified:** matches its original source.
- **Mismatch:** doesn't match, or an unexpected argument. Shown first, in red.
- **Can't be checked:** custom jars, and configs and scripts from content
  rules. Listed by path, with the jar's sha1. Scripts such as KubeJS are code
  and are listed separately from configs.
- **Source unreachable:** the original source couldn't be reached, so nothing
  is known yet.

## UI

- A **Verify** button per instance shows progress, then the results grouped
  by state.
- An optional **Check before every launch** setting runs the same check before
  each launch. A mismatch stops the launch and shows the results, with
  **Launch anyway**. Files that can't be checked or sources that can't be
  reached don't stop the launch.

Hashes of local files are cached by path, size and modification time. Answers
from the original sources are cached by hash, so repeated checks only ask about
new files.

## Out of scope

- **Launcher self-update.** If the launcher updates itself from the host, the
  host can replace the launcher. Verification assumes a trusted launcher
  binary. A later fix could require GitHub artifact attestations from public CI.
- Whether configs and scripts are safe.
- Tampering on the player's machine.

## Open questions

**CurseForge mods.** Options:

- a) Our own CurseForge API key, applied for at console.curseforge.com. The
  launcher computes CurseForge's fingerprint for each jar and sends them all to
  `/v1/fingerprints` in one request. Approval can take a while. Prism's key
  can't be reused: it isn't public, and using it would break CurseForge's API
  terms.
- b) No key. The launcher downloads the official file from
  `edge.forgecdn.net/files/<id/1000>/<id%1000>/<filename>` and compares hashes.
  The metadata has to carry each mod's CurseForge file ID and filename (the
  launcher still only trusts the hash comparison). That URL pattern isn't an
  official API, and every CurseForge jar gets downloaded again for each check.

Until this is settled, CurseForge mods are reported as "can't be checked".

**Forge/NeoForge patched jars.** Confirm that the installer's expected output
hashes cover every patched file. If they don't, the uncovered files are
reported as "can't be checked".

## Phases

1. Hashing and caching, scanning the instance folder, Mojang, Maven and
   Modrinth checks, results UI and the Verify button.
2. The launch argument check, Java and authlib-injector, Forge/NeoForge patched
   jars, check before every launch.
3. CurseForge, once the open question is settled.
