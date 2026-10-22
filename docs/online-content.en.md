# Online Mod and Modpack Content

`ccl-core::online_content::OnlineContentService` exposes provider search, compatible versions, dependency resolution, update checks, conflict analysis, and install operations without a CLI dependency.

## Providers

- Modrinth uses its public API for projects, versions, and required dependency resolution. Exact pinned dependency versions are retained and checked against the selected Minecraft version and loader. Optional dependencies are listed but not installed automatically; incompatible dependencies are reported during analysis.
- CurseForge uses its REST API. The caller must provide its own API key through `OnlineConfig`; the launcher does not include or persist the key. Download requests attach the key only to the exact HTTPS CurseForge CDN hosts allowed by the download request. If the API omits a file's download URL, that file is left unavailable; the launcher does not guess a website URL.

Search results can be filtered by project type, Minecraft version, and loader. Both providers expose version files with available hashes and sizes. CurseForge's numeric project IDs are retained as strings in the provider-neutral API.

## Instance operations

`install_project` selects a version compatible with the target instance, resolves required dependencies, downloads and verifies all selected files, then applies the set in one `InstanceService::edit_content` revision. It writes `.ccl/online-content.json`, recording project versions and SHA-512 ownership hashes. Other provider projects and local mods remain in place. Updates replace only files owned by the updated projects and remove obsolete owned files after checking that their bytes still match the receipt. A modified or missing owned file stops the update with a conflict so the active generation is preserved.

`updates_for_instance` checks receipt projects against their providers. `analyze_instance` uses the version metadata stored in the receipt and reports missing required dependencies, incompatible selections, duplicate project versions, and different files claiming the same filename. Modpack versions can be downloaded and passed to the existing Modrinth or CurseForge import converter.

## CLI examples

`online` accepts `modrinth` or `curseforge` as the provider. Project and version arguments can use the provider's project slug/ID and version ID. These examples show all seven subcommands:

```sh
ccl online search modrinth sodium --minecraft 1.21.1 --loader fabric --kind mod --limit 10
ccl online versions modrinth sodium --minecraft 1.21.1 --loader fabric
ccl online resolve modrinth sodium 1.21.1 fabric
ccl online install my-instance modrinth sodium --version VERSION_ID
ccl online updates my-instance
ccl online analyze my-instance
ccl online pack-install my-pack modrinth example-pack --version VERSION_ID --include-optional
```

`install` resolves and installs the selected project and required dependencies into an existing instance. `pack-install` downloads a provider pack and creates or updates an instance. `updates` and `analyze` inspect the receipt for an existing instance. CurseForge commands that call its API need `CCL_CURSEFORGE_API_KEY` in the process environment; Modrinth does not require a key. A successful fixture test of key header scoping does not establish live authorization for a real CurseForge key.

The APIs use `DownloadConfig` for proxy and offline behavior. A CurseForge key is required for CurseForge search, version lookup, and direct downloads; Modrinth needs no key. Online requests fail when offline mode is enabled.

## Validation

Unit fixtures cover provider search and version payload mapping, dependency types and pins, CurseForge file metadata and relationships, key redaction and header scoping, update selection, transactional ownership, rollback, and conflicts. A loopback HTTP fixture exercises Modrinth search, version listing, and recursive pinned dependency resolution. A separate opt-in test queries the public Modrinth API.
