# Instance lifecycle and retained revisions

These commands operate on managed instances under the selected game root. They use the same stable instance lock as launch; a running instance cannot be renamed, copied, deleted, restored or cleaned.

```sh
ccl instance copy source copy
ccl instance rename copy renamed
ccl instance history renamed
ccl instance disk-usage renamed
ccl instance restore renamed previous
ccl instance clean renamed --keep 2
ccl instance clean renamed --keep 2 --apply
ccl instance delete renamed --yes
```

Copy preserves the active instance, previous revision and archived revisions. Mutable files, worlds and configuration are independent copies. Verified asset files may share hard links. Copy does not copy unfinished stages. Rename changes the instance ID and rebases metadata paths; the operation records recovery information before moving the directory. IDs are ASCII letters, digits, `_` or `-`, up to 64 characters; case-only renames and case-insensitive destination collisions are refused.

Commits retain the immediate previous revision for `rollback`; older previous revisions are archived under `history/`. `history` lists `current`, `previous` (when available), and archive IDs. `restore ID REVISION` creates a new active revision from a complete retained snapshot, including its worlds and configuration, and retains the former active revision for rollback. `restore` accepts `previous` or an archive ID from `history`. Back up recent worlds before restoring. Loader rollback stages current saves, settings and other user data into the prior generation before swapping it into place; interruption recovery preserves a consistent active and rollback generation. Rollback after a full content edit or pack update swaps the complete snapshots.

`disk-usage` reports logical byte counts for active, previous, history and unfinished stage data. Hard links mean these numbers are not exclusive physical disk usage or guaranteed reclaimed space.

`clean` previews its targets by default. `--keep N` retains the newest N archived revisions (default 2); cleanup also targets an unfinished stage. `--include-previous` explicitly includes the immediate rollback revision. `--apply` executes the plan while holding the lock. Active worlds and content are never cleanup targets. Cleanup does not sweep shared root libraries/assets, other launchers' files, account credentials or installed Java runtimes in the active instance.

`delete --yes` permanently removes the entire selected instance, including its worlds and all history. Back up data first. It does not delete other instances or shared root resources. Stable lock files outside instance directories remain intentionally: deleting a lock file could allow two processes to bypass each other.
