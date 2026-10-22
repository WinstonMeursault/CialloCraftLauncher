# Importing CurseForge, MultiMC and Prism archives

`pack inspect ARCHIVE` inspects archives without downloading or installing. `pack import ID ARCHIVE` detects Modrinth, CurseForge and MultiMC/Prism exports by their manifests. A standard export may have its manifest at the ZIP root or inside one containing folder. Modrinth files need not have the `.mrpack` extension to be recognized.

```sh
ccl pack inspect ./export.zip
ccl pack import imported ./export.zip
ccl pack import imported ./export-v2.zip --replace-modified
```

## CurseForge

CCL reads Minecraft and loader versions from `manifest.json`, resolves pinned project/file IDs through the official API, verifies the declared SHA-1 and size, and imports the overrides and selected client files transactionally. Optional files are skipped unless `--include-optional` is supplied. File downloads use `x-api-key` only for explicitly recognized CurseForge HTTPS CDN hosts; the key is never added to archive URLs, receipts, or logs. Redirects to other permitted hosts do not receive it.

Remote file resolution requires your own API key in `CCL_CURSEFORGE_API_KEY`. Do not paste the key into a command argument or commit it. An overrides-only archive with no selected remote files can be converted without a key. A restricted file without a provider-authorized download URL fails with a diagnostic; CCL does not guess a download URL or bypass the author's distribution choice. The API and CDN requirements are documented by [CurseForge](https://docs.curseforge.com/rest-api/) and its [CDN authentication announcement](https://blog.curseforge.com/introducing-api-key-authentication-for-curseforge-file-downloads/).

This project has no owner-provided CurseForge API key for live acceptance. Provider behavior is checked with service/format fixtures; this is not evidence of production API authorization.

## MultiMC and Prism

CCL reads `mmc-pack.json` components and imports local game content from `minecraft/` or `.minecraft/`, including supported mods, configuration, packs, worlds, options and server lists. Loader/version requirements are resolved through CCL's existing installers. Exported instance hooks are not executed and account data is not imported. Unknown components and custom patches are rejected explicitly rather than silently producing a different instance. Multiple loaders are rejected. The current converter does not import arbitrary custom Java arguments, launcher-specific settings or account databases.

## Boundaries and recovery

Import uses the same locked, staged revision commit and file ownership conflict checks as Modrinth import. `--replace-modified` permits replacing conflicting managed content; back up local data before using it. Source archives remain untouched. Archive preflight checks entry count, declared uncompressed sizes, traversal, symlinks, case collisions and file/directory collisions before content is read. Limits are 100,000 entries, 128 MiB per entry, 512 MiB total uncompressed content and 2 MiB manifest JSON. CurseForge file metadata responses are also capped at 2 MiB, and cancellation interrupts pending metadata requests and downloads. Downloaded files count toward the converted pack's size limit.

These imports preserve private userdata, including worlds or server lists when present. Sharing an exported `.mrpack` is a separate action: that export excludes worlds/server lists and local binary redistribution still requires `--include-local-content`. Conversion does not grant redistribution permission.
