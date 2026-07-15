# rho-sdk

`rho-sdk` is the embeddable, headless agent runtime used by the Rho coding agent.
The crate is under active development toward its first stable release.

## Security defaults

The default feature set is empty. Creating an SDK runtime will not implicitly
read environment variables, access an operating-system credential store, write
to `~/.rho`, initialize a terminal or logger, check for updates, or grant tools
filesystem, process, or network access.

Capabilities such as built-in providers, SQLite persistence, keychain access,
web access, and coding tools will be introduced behind explicit adapters and
opt-in Cargo features as their public contracts are stabilized.

See [the Rho repository](https://github.com/matthewyjiang/rho) and
[the SDK tracking issue](https://github.com/matthewyjiang/rho/issues/256) for
the current roadmap.
