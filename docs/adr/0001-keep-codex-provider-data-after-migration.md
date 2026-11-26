# Keep Codex's old provider data after migrating it

Codex's **Sub2api providers** moved from `settings.json` (`sub2APIProvider`, with their cached accounts in `accounts.json` `cachedAccounts`) into `accounts.json` `providers`, stored and keyed by id like every other **Pool**'s. The migration copies and never deletes: the old `sub2APIProvider`, the old `cachedAccounts` and each provider's password under its bare id stay where they were, unread by this version once `codexProvidersMigrated` is set. An earlier version that finds no `sub2APIProvider` treats Codex as having no providers and, on its next settings save, empties `cachedAccounts`; keeping the old data lets a user downgrade without losing providers, cached accounts or passwords.

## Consequences

- The old data goes stale: changes made in an earlier version after the migration are not carried back into this one.
- Removing the old data is a later, deliberate step, taken once downgrading past this version is no longer supported; it is not dead code to clean up in passing.
