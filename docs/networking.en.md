# Networking and offline use

CCL separates the identity used to launch Minecraft from network availability. Setting `downloads.offline` to `true` disables network requests. A verified artifact already present at its normal game-directory path can be reused; a missing or invalid artifact produces an offline cache error.

Online installation saves the validated version manifest at `<game-directory>/.ccl/cache/version_manifest.json`. Offline installation uses this saved manifest and then checks the usual version metadata, libraries, asset index, and asset object paths. Offline mode cannot discover versions or download missing files. Return `downloads.offline` to `false` to refresh metadata or fill missing files.

The `versions` command also reads this validated per-game manifest in network offline mode, so it can list versions already known to that game directory without contacting a version service.

The install and launch cache lives in the selected game directory. Java runtime installation follows the same rule: an already valid managed runtime can be used without network access. A missing runtime can be installed offline when its saved runtime catalog, component manifest, and every required file are already cached; otherwise CCL reports the missing cache item. Existing source preferences and HTTP proxy settings apply only while online.

Downloads are checked against metadata hashes and sizes where provided. Interrupted single-file transfers can resume from a partial file only when the saved strong ETag matches the response ETag and the server confirms the requested offset and total with `Content-Range`. If those checks fail, CCL discards the partial and starts again. A file is published only after its complete size and hashes pass verification. Cancellation and exhausted retries leave the prior destination intact.
