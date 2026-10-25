# rust_xlsxwriter 0.99.1

Source: the locally cached crates.io package, without network access.
Upstream: https://github.com/jmcnamara/rust_xlsxwriter
Registry package checksum: `34f2d2cda7223168a14fb86bc7411dbc3379bd7d49749ecc53b91ed98585b6b0`.
License: MIT OR Apache-2.0; both original license files are retained.

The initial vendor commit copies every `src/` file byte-for-byte, together with
README.md, CHANGELOG.md (included by src/changelog.rs), Cargo.toml.orig and both
licenses. No examples, benchmarks, external test fixtures or generated artifacts
are copied. The active Cargo.toml is the registry-normalized manifest with only
explicit example/bench/test target tables removed. All package settings, library
target, features, optional dependencies, dependency versions and dev-dependencies
are retained. Original normalized manifest SHA-256:
`295a2251406ce2baaa17876746712a20a1b33d2e7ba758bf8f1ffecb45c36f03`.

Hedgehog patches this exact version through the workspace [patch.crates-io], using
the existing constant_memory and zmij features. The lockfile only changes the
source of rust_xlsxwriter; transitive dependency versions remain unchanged.

Subsequent changes, if accepted, must be reviewed separately from this pristine
source import. The vendor is not a workspace member. Public-API regressions belong
in core/tests so they run without the upstream examples or fixture suite.

The active experiment defers the first table's cell-occupancy map as one private
rectangle/index pair. A later registration reaching the original overlap check
materializes that map before running the unchanged subsequent-table loop. Pending
occupancy survives later autofilter errors; early validation errors do not touch
it. A worksheet saved with one table drops the pending pair without materializing
the map. This targets measured registration cost without changing table output.

SOURCE_SHA256.txt records the PRISTINE upstream files. The patched worksheet.rs
intentionally differs; review its isolated runtime commit against the pristine
vendor commit. Remove the experiment/vendor if exact outputs or measured useful
end-to-end improvement are not demonstrated.
