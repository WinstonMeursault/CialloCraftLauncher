# Modded instances and loaders

CCL can query loader metadata and install Fabric, Quilt, Legacy Fabric, Forge, NeoForge, and OptiFine into isolated managed instances. Loader compatibility is checked against the provider's version list before installation. This check confirms that the release is advertised for the requested Minecraft version; it does not verify every mod, modpack, Java setup, or launch combination.

Fabric and Legacy Fabric profiles come from their Meta APIs; Quilt profiles come from Quilt Meta. Forge supports processor based installers, spec 0 profiles with embedded Maven artifacts, and the earlier `install`/`versionInfo` universal JAR format. NeoForge uses processor based installers and embedded runtime artifacts. Unsupported installer structures return an explicit unsupported-format error.

OptiFine releases are listed through the configured BMCLAPI compatible mirror. CCL downloads a fresh installer over HTTPS, checks its archive size and the embedded Minecraft/OptiFine version constants, and runs the bundled patcher against the vanilla client JAR when the installer contains one. Bundled LaunchWrapper JARs are installed when present; older installers fall back to Maven LaunchWrapper 1.12. OptiFine does not publish a checksum through this metadata endpoint, so CCL does not reuse a prior installer download as a verified cache entry.

Isolated cold installs and macOS game menus have been verified for Quilt `0.20.0-beta.9` on Minecraft `1.21.4`, NeoForge `21.1.234` on Minecraft `1.21.1`, and OptiFine `HD_U_I6` on Minecraft `1.20.1`. Legacy Fabric `0.20.1` on Minecraft `1.8.9` and Forge `14.23.5.2860` on Minecraft `1.12.2` installed successfully on macOS and Linux; Linux clients reached game startup using a virtual display and software rendering. These results do not certify other versions, platforms, or third-party mods.

## Query loader releases

```sh
ccl loaders fabric 1.21.1
ccl loaders quilt 1.21.4
ccl loaders legacyfabric 1.8.9
ccl loaders forge 1.20.1 --limit 10
ccl loaders neoforge 1.21.1 --limit 10
ccl loaders optifine 1.20.1
```

The command prints compatible loader releases, newest first. Pass the selected release string to `instance install` or `instance update`.

## Manage an isolated instance

```sh
ccl instance install my-world 1.21.1 fabric 0.20.1
ccl instance install my-quilt-world 1.21.4 quilt 0.20.0-beta.9
ccl instance install my-optifine-world 1.20.1 optifine HD_U_I6
ccl instance list
ccl instance show my-world
ccl launch --instance my-world --dry-run
ccl logs --instance my-world
ccl instance update my-world 1.21.1 fabric 0.20.1
ccl instance rollback my-world
ccl instance uninstall-loader my-world
```

Each managed instance has its own game directory, version metadata, mods, configuration, and saves. It does not share the selected vanilla game directory with another instance. Installing or updating builds a staging revision and switches to it after a successful install. Updates retain the previous successful revision for `rollback`. Rollback switches to that revision. Loader removal returns the instance to vanilla while retaining user game data. These operations preserve mods, configuration, and saves across revisions.

The `loader_version` field shown by `instance show` records the resolved installed profile ID, such as `fabric-loader-0.20.1-1.21.1`; it is not necessarily the exact release string supplied on the command line. Instance launch uses the selected installed profile and its separate game directory.

CCL does not bundle Fabric API, mods, or modpacks. Install those separately into the instance's `mods` directory. Upstream compatibility metadata is a preflight check, not a guarantee that a particular mod set will work.

An instance selects one loader at a time. Use mods built for that loader and Minecraft release; Fabric, Forge and NeoForge mod files are not generally interchangeable. Changing the loader preserves the existing `mods` directory, so review those files before launching with the new loader. Removing the loader leaves those files in place but vanilla does not load them.

Forge-family installer integrity is checked against the official Maven SHA-1 sidecar. If it is unavailable, CCL fetches the official SHA-512 sidecar, verifies the installer bytes against it, and only then derives the SHA-1 used by the install pipeline. A cached installer is reused only when its URL, recorded SHA-1, and file size match. If neither upstream checksum can be obtained and no valid matching record exists, installation fails. Offline installation can reuse a verified installer record when the other required artifacts are already available; loader version discovery requires network access.

Metadata and checksum sources: [Fabric Meta](https://meta.fabricmc.net/), [Quilt Meta](https://meta.quiltmc.org/), [Legacy Fabric Meta API](https://github.com/Legacy-Fabric/legacy-meta), [OptiFine downloads](https://optifine.net/downloads), [Forge Maven](https://maven.minecraftforge.net/), and [NeoForge Maven](https://maven.neoforged.net/releases/).

`--json` can be used with these commands. `loaders` returns `loader_versions`; `instance list`, `show`, and lifecycle commands return `managed_instances` or `instance` records. Network and installation errors follow the general [CLI exit codes](cli.en.md#exit-codes).
