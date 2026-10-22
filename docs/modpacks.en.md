# Modrinth packs and instance content

CCL can inspect, import and export Modrinth .mrpack v1 archives, including Quilt dependencies, and manage isolated instance content. CurseForge and MultiMC/Prism exports are supported through [additional import converters](imports.en.md). A GUI is not provided.

The validated sample uses Minecraft `1.21.1`, Fabric Loader `0.20.1`, and Fabric API `0.116.17+1.21.1`. It reached the main menu on macOS arm64 and on Linux x86_64 using CPU rendering. An exported and re-imported macOS instance also reached the menu with matching mod hashes and override files. This does not certify other packs or third-party mod combinations. Windows is checked through compilation and automated tests.

## Inspect and import a pack

```sh
ccl pack inspect ./pack.mrpack
ccl pack import my-pack ./pack.mrpack
ccl pack show my-pack
```

`inspect` validates the archive and prints its manifest without installing it. `show` prints the recorded manifest and ownership data for an imported instance. Import supports Minecraft with no loader, Fabric, Forge, or NeoForge as declared by the pack. Optional client files are skipped unless requested:

```sh
ccl pack import my-pack ./pack.mrpack --include-optional
```

Importing another archive into an existing managed ID updates that instance. CCL checks that pack-owned files still match their recorded hashes. A locally changed or deleted owned file, or a conflicting unmanaged file at a new pack path, stops the update with exit code `7`. Back up local work, or explicitly discard those changes with:

```sh
ccl pack import my-pack ./pack-v2.mrpack --replace-modified
```

`--replace-modified` permits replacing or removing the conflicting local files as needed by the new pack. Treat it as permission to discard those local changes.

## Download sources and integrity

Pack file URLs must use HTTPS. By default, downloads are accepted from Modrinth's CDN, GitHub, and GitLab hosts, with GitHub release asset hosts allowed for redirects. Every redirect is checked against the same HTTPS host policy, with a maximum of 10 redirects. Add a trusted exact hostname when a pack uses another source:

```sh
ccl pack import my-pack ./pack.mrpack --allow-host downloads.example.org
```

Repeat `--allow-host` to add more hosts. CCL checks each downloaded file's SHA-1, SHA-512, and declared size before placing it in the instance. `.mrpack` archives are limited to 100,000 entries, 128 MiB per file, and 512 MiB total uncompressed data.

## Export a pack

```sh
ccl pack export my-pack ./my-pack.mrpack
```

By default, export keeps verified remote file references and includes local configuration and options as overrides. It does not upload anything. Files that differ from the imported pack cannot be represented by their old remote reference. New or untracked mod, resource-pack, and shader-pack binaries require the explicit `--include-local-content` option:

Export also accepts ordinary managed instances created with `instance install`, including vanilla instances. It derives Minecraft and loader dependencies from the installed instance metadata. An instance without a pack record has no pinned remote references, so exporting its local binaries requires the same explicit permission option.

```sh
ccl pack export my-pack ./my-pack.mrpack --include-local-content
```

Use that option only when you have permission to redistribute every included binary. CCL does not check licenses or obtain permission for you. Review Modrinth's [`.mrpack` format specification](https://support.modrinth.com/en/articles/8802351-modrinth-modpack-format-mrpack) and [modpack permissions guidance](https://support.modrinth.com/en/articles/8797527-obtaining-modpack-permissions) before sharing an archive. CCL does not publish or upload the exported file.

Saves and `servers.dat` are excluded from pack exports; use a userdata backup for them.

## Manage local content

```sh
ccl content list my-pack mods
ccl content list my-pack resourcepacks
ccl content list my-pack shaderpacks
ccl content list my-pack saves
ccl content add my-pack mods ./local-mod.jar
ccl content add my-pack saves ./world --name MyWorld
ccl content remove my-pack mods local-mod.jar
ccl content enable my-pack example.jar
ccl content disable my-pack example.jar
```

`content add` copies the source and leaves the original in place. Mods must be `.jar` files (or `.jar.disabled` files); resource packs and shader packs must be `.zip` files; saves must be directories. `--name` is optional and selects the destination name. Enable and disable apply to `.jar` mods only.

## Back up and restore userdata

```sh
ccl content backup my-pack ./my-pack-userdata.zip
ccl content restore my-pack ./my-pack-userdata.zip
```

Backups contain user data from `mods`, `config`, `resourcepacks`, `shaderpacks`, `saves`, `defaultconfigs`, `datapacks`, `options.txt`, and `servers.dat`. They do not contain Minecraft or loader files, Java runtimes, or account credentials. Restore replaces every declared root; roots that were absent in the backup are removed from the instance. Make a backup before restoring if you may need the current files. Backups are limited to 100,000 entries and 2 GiB of file data.

## Revisions and running instances

Pack imports/updates and content edits are prepared in a staged instance generation and committed under the instance operation lock. They cannot replace the game directory while a managed launch holds the lock. `instance rollback` after a pack or content change restores the exact previous snapshot, including its user data and saves. Back up first if you need those newer saves. Loader rollback preserves current user data while switching loader revisions.

See [CLI exit codes](cli.en.md#exit-codes); these commands support `--json`. Standard Modrinth dependencies cannot represent standalone OptiFine or Legacy Fabric; those managed instances use userdata backup rather than a misleading portable export.
