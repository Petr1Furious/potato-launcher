# Servers

A modded server is another target of the pack repo, like Fjord. potato keeps
only the shared parts in sync: mods with `side` `server` or `both`, and the
managed paths from `[[paths]]`. Everything else stays the server's own and
never enters the repo: `server.properties`, server-only mods and their
configs, permissions, the world, ops and whitelist.

Paper and other plugin servers aren't supported.

## Setup

potato runs on the server host, against a clone of the pack repo:

```bash
git clone <pack repo> ~/packs/minigames
cd ~/packs/minigames
```

Point it at the server folder in `.potato/local.toml` (not committed):

```toml
[servers]
main = "/srv/minecraft/minigames"
```

Install the loader the way your server runtime does it (for example, the
`itzg/minecraft-server` image with `TYPE=NEOFORGE`). potato doesn't install
loaders.

The first push overwrites the managed files on the server:

```bash
potato push main --force
```

## Updating the server

```bash
git pull
# stop the server
potato push main
# start the server
```

`push` refuses while the server is running (the world's `session.lock` is
held; not detected on Windows hosts). `--force` skips the checks. It writes the managed files from the last commit, downloads missing
jars, and deletes only files and jars that the last sync had and the repo has
since removed. Jars and configs potato never wrote are left alone.

## Editing files on the server

If you change a managed file directly on the server, the next `push` refuses
until you bring the change into the repo:

```bash
potato pull main
git diff --cached
git commit && git push
potato push main
```

Pulling from a server only takes changes to files the repo already has. New
files in managed folders are ignored, since they're almost always configs of
server-only mods. Take one anyway with `--add`:

```bash
potato pull main --add config/newmod.toml
```

## Server-only mods

Drop the jar and its config into the server folder. potato never touches
them. To track one in the repo instead:

```bash
potato add <file or URL> --side server
```

## A shared file the server keeps its own version of

Add it to that server's `exclude` in `potato.toml`:

```toml
[servers.main]
exclude = ["config/somemod.toml"]
```

potato then never writes or reads it on that server. If the file is missing,
the mod writes its defaults on first start, and you edit it on the server.

## Several servers

Each server gets its own name in `.potato/local.toml` and its own
`[servers.<name>]` section.
