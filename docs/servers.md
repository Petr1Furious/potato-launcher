# Servers

A modded server is another target of the pack repo. potato only keeps the shared parts in sync: mods with `side` set to `server` or `both`, and the managed paths from `[[paths]]`. Everything else (such as `server.properties`) belongs to the server and never enters the repo.

It is only possible to push to servers. The idea behind this is that changes start in Prism, go into the repo, and then onto the server.

::: info
Paper and other plugin servers aren't supported, because usually there is nothing to keep in sync between the clients and the server.
:::

## Setup

potato runs on the server host, against a clone of the pack repo.

1. Clone the repo:

```bash
git clone <pack repo> ~/packs/minigames
cd ~/packs/minigames
```

2. Add the server folder as a target. You can pick any name:

```bash
potato target add main /srv/minecraft/minigames
```

Targets are stored in `.potato/local.toml`, which isn't committed.

3. Install the loader the way your server runtime does it, for example with the `itzg/minecraft-server` image and `TYPE=NEOFORGE`. potato doesn't install loaders.

4. Push the pack:

```bash
potato push main
```

If files on the server were changed there, push refuses and names them. Copy the edits you want to keep into the repo, then run `potato push main --force`; or add the files to `exclude`.

## Updating the server

```bash
git pull
# stop the server
potato push main
# start the server
```

`push` writes the managed files from the last commit, downloads missing jars, and deletes only the files and jars that the last push had and the repo has since removed. Jars and configs potato never wrote are left alone.

`push` refuses to run while the server is running, which it detects by the world's `session.lock`. This doesn't work on Windows hosts. `--force` skips this check and the one for edits made on the server.

## Editing files on the server

If you change a managed file directly on the server, the next `push` refuses and names it. Copy the change into the repo, commit and push again, or run `push --force` to drop it.

## Server-only mods

Put the jar and its config into the server folder, and potato won't touch them. You can also track one in the repo:

```bash
potato mod add <file or URL> --side server
```

## A shared file the server keeps its own version of

Add it to that server's `exclude` in `potato.toml`:

```toml
[servers.main]
exclude = ["config/somemod.toml"]
```

potato then never writes it on that server. If the file is missing, the mod writes its defaults on first start, and you can edit it on the server.

## Several servers

Add each server as a target with its own name (`potato target add <name> <folder>`). Its `exclude` list goes in its own `[servers.<name>]` section of `potato.toml`.
