#!/usr/bin/env python3
"""Remote instance sync/build helper for Potato Launcher.

This script intentionally uses only Python's standard library. It still
requires `ssh` and `rsync` locally, plus Docker on the remote host for builds.
"""

import argparse
import json
import os
import shlex
import shutil
import subprocess
import sys
import tempfile


DEFAULT_CONFIG = os.path.join(os.path.dirname(__file__), "remote-instance.json")
DEFAULT_EXCLUDES = [".git", "saves"]
DEFAULT_CONTAINER = "potato-launcher-backend"
DEFAULT_CONTAINER_SPEC = "/data/internal/spec.json"
DEFAULT_CONTAINER_GENERATED = "/data/generated"
DEFAULT_CONTAINER_WORKDIR = "/data/workdir"
DEFAULT_CONTAINER_UPLOADED_INSTANCES = "/data/internal/uploaded-instances"


def log(message):
    print(message, file=sys.stderr)


def warn(message):
    log("warning: {0}".format(message))


def die(message):
    log("error: {0}".format(message))
    sys.exit(1)


def shell_join(parts):
    return " ".join(shlex.quote(str(part)) for part in parts)


def require_tool(name):
    if shutil.which(name) is None:
        die("{0} is required on your local machine".format(name))


def load_config(path):
    if path is None:
        path = DEFAULT_CONFIG if os.path.exists(DEFAULT_CONFIG) else None
    if path is None:
        return {}
    try:
        with open(path, "r") as file_obj:
            data = json.load(file_obj)
    except IOError as err:
        die("failed to read config {0}: {1}".format(path, err))
    except ValueError as err:
        die("failed to parse config {0}: {1}".format(path, err))
    if not isinstance(data, dict):
        die("config must be a JSON object: {0}".format(path))
    return data


def cfg_value(args, config, attr, key, default=None):
    value = getattr(args, attr, None)
    if value is not None:
        return value
    return config.get(key, default)


def cfg_bool(args, config, attr, key, default=False):
    value = getattr(args, attr, None)
    if value is not None:
        return value
    return bool(config.get(key, default))


def cfg_container_path(config, name, default):
    paths = config.get("container_paths", {})
    if not isinstance(paths, dict):
        die("config field 'container_paths' must be an object")
    return paths.get(name, default)


def parse_instance_mapping(raw):
    if "=" not in raw:
        die("bad instance mapping '{0}': expected ID=DIR".format(raw))
    instance_id, path = raw.split("=", 1)
    instance_id = instance_id.strip()
    path = path.strip()
    if not instance_id:
        die("bad instance mapping '{0}': empty id".format(raw))
    if not path:
        die("bad instance mapping '{0}': empty path".format(raw))
    return instance_id, path


def config_instances(config):
    raw = config.get("instances", {})
    if isinstance(raw, dict):
        return dict((str(instance_id), str(path)) for instance_id, path in raw.items())
    if isinstance(raw, list):
        return dict(parse_instance_mapping(str(item)) for item in raw)
    if raw in (None, ""):
        return {}
    die("config field 'instances' must be an object or list")


def merged_instances(args, config):
    result = config_instances(config)
    for raw in args.instance or []:
        instance_id, path = parse_instance_mapping(raw)
        result[instance_id] = path
    return result


def merged_excludes(args, config):
    excludes = []
    if not args.no_default_excludes:
        excludes.extend(DEFAULT_EXCLUDES)
    cfg_excludes = config.get("exclude", [])
    if isinstance(cfg_excludes, str):
        cfg_excludes = [cfg_excludes]
    if not isinstance(cfg_excludes, list):
        die("config field 'exclude' must be a string or list")
    excludes.extend(str(item) for item in cfg_excludes)
    excludes.extend(args.exclude or [])

    deduped = []
    seen = set()
    for item in excludes:
        if item not in seen:
            deduped.append(item)
            seen.add(item)
    return deduped


def ensure_absolute(path, flag_name):
    if not path:
        die("{0} is required".format(flag_name))
    if not os.path.isabs(path):
        die("{0} must be an absolute path (got: {1})".format(flag_name, path))


def remote_path(remote, path):
    return "{0}:{1}".format(remote, shlex.quote(path))


def run_command(command, dry_run=False, execute_dry_run=False):
    if dry_run:
        log("[dry-run] {0}".format(shell_join(command)))
        if not execute_dry_run:
            return
    subprocess.check_call(command)


def remote_dir_exists(args, config, remote, path):
    port = str(cfg_value(args, config, "ssh_port", "ssh_port", 22))
    command = ["ssh", "-p", port, remote, "test", "-d", path]
    if args.dry_run:
        log("[dry-run] {0}".format(shell_join(command)))
    result = subprocess.call(command, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    return result == 0


def rsync_supports_progress2():
    try:
        result = subprocess.Popen(
            ["rsync", "--info=progress2", "--version"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        result.communicate()
    except OSError:
        return False
    return result.returncode == 0


def rsync_progress_args():
    if rsync_supports_progress2():
        return ["--info=progress2"]
    warn(
        "your rsync does not support --info=progress2; falling back to --progress. "
        "On macOS, `brew install rsync` provides a newer rsync."
    )
    return ["--progress"]


def rsync_base(args, config, delete=False):
    port = str(cfg_value(args, config, "ssh_port", "ssh_port", 22))
    command = ["rsync", "-az", "-e", "ssh -p {0}".format(port)]
    if delete:
        command.append("--delete")
    command.extend(rsync_progress_args())
    for pattern in merged_excludes(args, config):
        command.extend(["--exclude", pattern])
    if args.dry_run:
        command.append("--dry-run")
    return command


def config_env(config):
    raw = config.get("env", {})
    if raw in (None, ""):
        return {}
    if not isinstance(raw, dict):
        die("config field 'env' must be an object")
    result = {}
    for key, value in raw.items():
        if not isinstance(key, str) or not key:
            die("config env keys must be non-empty strings")
        if value is None:
            continue
        if not isinstance(value, str):
            die("config env value for '{0}' must be a string".format(key))
        result[key] = value
    return result


def docker_exec_env_args(config):
    args = []
    for key, value in sorted(config_env(config).items()):
        args.extend(["-e", "{0}={1}".format(key, value)])
    return args


def rewrite_spec_source_roots(spec_path, source_root_base):
    try:
        with open(spec_path, "r") as file_obj:
            spec = json.load(file_obj)
    except IOError as err:
        die("failed to read spec {0}: {1}".format(spec_path, err))
    except ValueError as err:
        die("failed to parse spec {0}: {1}".format(spec_path, err))

    instances = spec.get("instances")
    if not isinstance(instances, list):
        die("spec field 'instances' must be an array")

    base = source_root_base.rstrip("/")
    for index, instance in enumerate(instances):
        if not isinstance(instance, dict):
            die("spec instance at index {0} must be an object".format(index))
        instance_id = instance.get("id")
        if not instance_id:
            die("spec instance at index {0} is missing id".format(index))
        intended = "{0}/{1}".format(base, instance_id)
        current = instance.get("source_root")
        if current and current != intended:
            warn(
                "replacing source_root for '{0}': {1} -> {2}".format(
                    instance_id, current, intended
                )
            )
        instance["source_root"] = intended

    temp = tempfile.NamedTemporaryFile(
        mode="w", prefix="potato-spec-", suffix=".json", delete=False
    )
    try:
        json.dump(spec, temp, indent=4)
        temp.write("\n")
        return temp.name
    finally:
        temp.close()


def build_command(args, config):
    remote = cfg_value(args, config, "remote", "remote")
    internal_dir = cfg_value(args, config, "internal_dir", "internal_dir")
    ensure_absolute(internal_dir, "--internal-dir")
    if not remote:
        die("--remote is required (or set it in config)")

    spec = cfg_value(args, config, "spec", "spec")
    instances = merged_instances(args, config)
    do_build = not args.no_build

    if not spec and not instances and not do_build:
        die("nothing to do: provide --spec and/or --instance, or omit --no-build")

    require_tool("rsync")
    require_tool("ssh")

    remote_spec_host = os.path.join(internal_dir.rstrip("/"), "spec.json")
    if spec:
        if not os.path.isfile(spec):
            die("spec file not found: {0}".format(spec))
        source_root_base = cfg_value(
            args,
            config,
            "container_uploaded_instances",
            "container_uploaded_instances",
            DEFAULT_CONTAINER_UPLOADED_INSTANCES,
        )
        temp_spec = rewrite_spec_source_roots(spec, source_root_base)
        try:
            log("Syncing rewritten spec -> {0}:{1}".format(remote, remote_spec_host))
            command = rsync_base(args, config, delete=False)
            command.extend([temp_spec, remote_path(remote, remote_spec_host)])
            run_command(command, dry_run=args.dry_run, execute_dry_run=True)
        finally:
            try:
                os.unlink(temp_spec)
            except OSError:
                pass

    for instance_id, local_dir in instances.items():
        if not os.path.isdir(local_dir):
            die("instance dir not found for '{0}': {1}".format(instance_id, local_dir))
        remote_instance_host = "{0}/uploaded-instances/{1}/".format(
            internal_dir.rstrip("/"), instance_id
        )
        local_source = local_dir.rstrip("/") + "/"
        log(
            "Syncing instance '{0}' ({1}) -> {2}:{3}".format(
                instance_id, local_dir, remote, remote_instance_host
            )
        )
        command = rsync_base(args, config, delete=True)
        command.extend([local_source, remote_path(remote, remote_instance_host)])
        run_command(command, dry_run=args.dry_run, execute_dry_run=True)

    if do_build:
        container = cfg_value(args, config, "container", "container", DEFAULT_CONTAINER)
        docker_host = cfg_value(args, config, "docker_host", "docker_host", "")
        container_spec = cfg_value(
            args,
            config,
            "container_spec",
            "container_spec",
            cfg_container_path(config, "spec", DEFAULT_CONTAINER_SPEC),
        )
        container_generated = cfg_value(
            args,
            config,
            "container_generated",
            "container_generated",
            cfg_container_path(config, "generated", DEFAULT_CONTAINER_GENERATED),
        )
        container_workdir = cfg_value(
            args,
            config,
            "container_workdir",
            "container_workdir",
            cfg_container_path(config, "workdir", DEFAULT_CONTAINER_WORKDIR),
        )
        docker_command = ["docker", "exec"]
        docker_command.extend(docker_exec_env_args(config))
        docker_command.extend(
            [
                container,
                "instance-builder",
                "-s",
                container_spec,
                container_generated,
                container_workdir,
            ]
        )
        remote_command = shell_join(docker_command)
        if docker_host:
            remote_command = "DOCKER_HOST={0} {1}".format(
                shlex.quote(docker_host), remote_command
            )
        ssh_command = [
            "ssh",
            "-p",
            str(cfg_value(args, config, "ssh_port", "ssh_port", 22)),
            remote,
            remote_command,
        ]
        log("Triggering remote build via docker exec in container: {0}".format(container))
        run_command(ssh_command, dry_run=args.dry_run)
    else:
        log("Skipping build (--no-build).")

    log("Done.")


def fetch_command(args, config):
    remote = cfg_value(args, config, "remote", "remote")
    internal_dir = cfg_value(args, config, "internal_dir", "internal_dir")
    ensure_absolute(internal_dir, "--internal-dir")
    if not remote:
        die("--remote is required (or set it in config)")

    spec_out = cfg_value(args, config, "spec_out", "spec_out")
    instances = merged_instances(args, config)
    if not spec_out and not instances:
        die('nothing to fetch: use --spec-out PATH and/or --instance "ID=DIR"')

    require_tool("rsync")
    require_tool("ssh")

    if spec_out:
        remote_spec = os.path.join(internal_dir.rstrip("/"), "spec.json")
        parent = os.path.dirname(spec_out)
        if parent:
            os.makedirs(parent, exist_ok=True)
        log("Fetching spec -> {0}".format(spec_out))
        command = rsync_base(args, config, delete=False)
        command.extend([remote_path(remote, remote_spec), spec_out])
        run_command(command, dry_run=args.dry_run, execute_dry_run=True)

    for instance_id, local_dir in instances.items():
        os.makedirs(local_dir, exist_ok=True)
        remote_instance_dir = "{0}/uploaded-instances/{1}/".format(
            internal_dir.rstrip("/"), instance_id
        )
        remote_instance_path = remote_instance_dir.rstrip("/")
        if not remote_dir_exists(args, config, remote, remote_instance_path):
            warn(
                "remote instance dir not found for '{0}': {1}:{2} (skipping)".format(
                    instance_id, remote, remote_instance_path
                )
            )
            continue
        log("Fetching instance '{0}' -> {1}/".format(instance_id, local_dir))
        command = rsync_base(args, config, delete=args.delete)
        command.extend([remote_path(remote, remote_instance_dir), local_dir.rstrip("/") + "/"])
        run_command(command, dry_run=args.dry_run, execute_dry_run=True)

    log("Done.")


def add_common_options(parser):
    parser.add_argument("--remote", help="SSH destination, for example user@host")
    parser.add_argument("--ssh-port", type=int, help="SSH port (default: 22)")
    parser.add_argument(
        "--internal-dir",
        help="Remote absolute path to the backend internal directory",
    )
    parser.add_argument(
        "--instance",
        action="append",
        help='Instance mapping ID=DIR. Repeatable. Overrides same config id.',
    )
    parser.add_argument(
        "--exclude",
        action="append",
        help="Rsync exclude pattern. Repeatable. Added after config/default excludes.",
    )
    parser.add_argument(
        "--no-default-excludes",
        action="store_true",
        help="Do not add default excludes (.git, saves)",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Print commands and pass --dry-run to rsync",
    )


def build_parser():
    parser = argparse.ArgumentParser(
        description="Sync Potato Launcher instance files and run remote builds.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""Examples:
  # Sync spec + modpacks, then build
  python3 scripts/remote-instance.py build --remote user@host --internal-dir /abs/path/to/internal --spec ./spec.json \\
    --instance "minigames=./packs/minigames" \\
    --instance "skyblock=./packs/skyblock"

  # Use JSON config
  python3 scripts/remote-instance.py --config scripts/remote-instance.json build

  # Fetch spec + selected uploaded files
  python3 scripts/remote-instance.py fetch --spec-out ./spec.json --instance "minigames=./packs/minigames"
""",
    )
    parser.add_argument(
        "-c",
        "--config",
        help="JSON config path (default: scripts/remote-instance.json if it exists)",
    )

    subparsers = parser.add_subparsers(dest="command")

    build = subparsers.add_parser(
        "build",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        help="sync spec/files and run instance-builder remotely",
        epilog="""Notes:
  - Spec source_root values are rewritten only in a temporary uploaded copy.
  - Default excludes are .git and saves. Use --no-default-excludes to disable them.
  - Optional config "env" object passes variables to remote docker exec (e.g. HTTP_PROXY).
""",
    )
    build.add_argument(
        "-c",
        "--config",
        help="JSON config path (default: scripts/remote-instance.json if it exists)",
    )
    add_common_options(build)
    build.add_argument("--container", help="Remote Docker container name")
    build.add_argument(
        "--docker-host",
        help="Remote DOCKER_HOST value, e.g. unix:///run/user/1000/docker.sock",
    )
    build.add_argument("--spec", help="Local spec.json path")
    build.add_argument("--container-spec", help="Spec path inside container")
    build.add_argument("--container-generated", help="Generated dir inside container")
    build.add_argument("--container-workdir", help="Workdir inside container")
    build.add_argument(
        "--container-uploaded-instances",
        help="In-container uploaded instances base used for source_root rewrite",
    )
    build.add_argument("--no-build", action="store_true", help="Only sync, no docker exec")

    fetch = subparsers.add_parser(
        "fetch",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        help="fetch spec/files from the remote backend internal directory",
        epilog="""Notes:
  - By default, fetch does not delete local files missing on the remote.
  - Use --delete to mirror remote uploaded files locally.
""",
    )
    fetch.add_argument(
        "-c",
        "--config",
        help="JSON config path (default: scripts/remote-instance.json if it exists)",
    )
    add_common_options(fetch)
    fetch.add_argument(
        "--spec-out",
        help="Download <internal-dir>/spec.json and write it to this path",
    )
    fetch.add_argument(
        "--delete",
        action="store_true",
        help="Delete local files that are not present on the remote",
    )

    return parser


def main(argv=None):
    parser = build_parser()
    args = parser.parse_args(argv)
    if not args.command:
        parser.print_help()
        return 2

    config = load_config(args.config)
    if args.command == "build":
        build_command(args, config)
    elif args.command == "fetch":
        fetch_command(args, config)
    else:
        parser.error("unknown command: {0}".format(args.command))
    return 0


if __name__ == "__main__":
    sys.exit(main())
