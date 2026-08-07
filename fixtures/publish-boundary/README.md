# Publish-boundary fixture

Synthetic crates that model the registry-boundary failure caught by
`scripts/crate_publish_prep.py`.

## Layout

- `registry/boundary-dep` - same version as the workspace dep, without
  `EXTRA_SYMBOL` (the API surface already on a registry)
- `workspace/boundary-dep` - same version with `EXTRA_SYMBOL` (local sources that
  moved an export without a version bump)
- `consumer` - depends on `boundary-dep = "=0.1.0"` and imports `EXTRA_SYMBOL`

The exact `=` pin keeps Cargo from floating to a newer crates.io release if a
real `boundary-dep` crate ever appears.

## Expected behavior

1. Patch the consumer onto `registry/boundary-dep` and `cargo check` fails with a
   missing `EXTRA_SYMBOL` import.
2. Patch the consumer onto `workspace/boundary-dep` and `cargo check` succeeds.
3. Publish prep path-patch policy must omit the workspace patch when that exact
   dependency version is already on crates.io, so verification sees the registry
   surface and fails the same way as a real publish.

Run:

```bash
python3 scripts/crate_publish_prep.py --self-test
```
