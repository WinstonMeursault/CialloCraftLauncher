# Modded instances and loaders

CCL can query upstream loader metadata and install Fabric, Forge, and NeoForge into isolated managed instances. Loader compatibility is checked against the upstream version list before installation. This check confirms that the loader release is advertised for the requested Minecraft version; it does not verify every mod, modpack, Java setup, or real-game launch combination.

Fabric profiles come from Fabric Meta. Forge and NeoForge use their supported modern installer formats. Legacy installer formats that the current installer path cannot process return an unsupported-format error.

The Linux validation run installed Minecraft `1.21.1` with Fabric `0.20.1`, reached the Minecraft main menu, and exited with code `0`. This confirms that one real Fabric combination launches; it does not certify other versions, platforms, Forge/NeoForge, or third-party mods.

## Query loader releases

```sh
ccl loaders fabric 1.21.1
ccl loaders forge 1.20.1 --limit 10
ccl loaders neoforge 1.21.1 --limit 10
```

The command prints compatible loader releases, newest first. Pass the selected release string to `instance install` or `instance update`.

## Manage an isolated instance

```sh
ccl instance install my-world 1.21.1 fabric 0.20.1
ccl instance list
ccl instance show my-world
ccl launch --instance my-world --dry-run
ccl instance update my-world 1.21.1 fabric 0.20.1
ccl instance rollback my-world
ccl instance uninstall-loader my-world
```

Each managed instance has its own game directory, version metadata, mods, configuration, and saves. It does not share the selected vanilla game directory with another instance. Installing or updating builds a staging revision and switches to it after a successful install. Updates retain the previous successful revision for `rollback`. Rollback switches to that revision. Loader removal returns the instance to vanilla while retaining user game data. These operations preserve mods, configuration, and saves across revisions.

The `loader_version` field shown by `instance show` records the resolved installed profile ID, such as `fabric-loader-0.20.1-1.21.1`; it is not necessarily the exact release string supplied on the command line. Instance launch uses the selected installed profile and its separate game directory.

CCL does not bundle Fabric API, mods, or modpacks. Install those separately into the instance's `mods` directory. Upstream compatibility metadata is a preflight check, not a guarantee that a particular mod set will work.

`--json` can be used with these commands. `loaders` returns `loader_versions`; `instance list`, `show`, and lifecycle commands return `managed_instances` or `instance` records. Network and installation errors follow the general [CLI exit codes](cli.en.md#exit-codes).
